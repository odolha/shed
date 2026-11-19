//! Setting up the account, signing in, sessions and devices.

mod common;

use common::*;
use reqwest::StatusCode;
use shed_api::ErrorCode;
use shed_api::http::*;

#[tokio::test]
async fn setup_makes_the_account_and_signs_in() {
    let nexus = start().await;
    let status: NexusStatus = nexus.get("/api/v1/status", &Auth::None).await;
    assert!(!status.ready && !status.signed_in);
    let code = nexus.server.setup_code.clone().expect("a setup code while there is no account");

    let wrong = SetupStart { code: "nope".into(), username: "me".into(), password: PASSWORD.into() };
    let err = nexus.post_err("/api/v1/setup", &wrong, &Auth::None, StatusCode::UNAUTHORIZED).await;
    assert_eq!(err.code, ErrorCode::Unauthorized);
    let short = SetupStart { code: code.clone(), username: "me".into(), password: "short".into() };
    nexus.post_err("/api/v1/setup", &short, &Auth::None, StatusCode::BAD_REQUEST).await;

    let start = SetupStart { code: code.clone(), username: "me".into(), password: PASSWORD.into() };
    let totp: SetupTotp = nexus.post("/api/v1/setup", &start, &Auth::None).await;
    assert!(totp.url.starts_with("otpauth://totp/"), "{}", totp.url);
    let mut auth = Authenticator::new(&totp.secret);

    let bad = SetupConfirm { code: code.clone(), totp: "000000".into(), device: browser() };
    let err = nexus.post_err("/api/v1/setup/confirm", &bad, &Auth::None, StatusCode::FORBIDDEN).await;
    assert_eq!(err.code, ErrorCode::WrongCode);

    let confirm = SetupConfirm { code: code.clone(), totp: auth.code().await, device: browser() };
    let answer = nexus.post_raw("/api/v1/setup/confirm", &confirm, &Auth::None).await;
    assert_eq!(answer.status(), StatusCode::OK);
    let cookie = answer.headers()["set-cookie"].to_str().unwrap().to_string();
    for part in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(part), "{cookie}");
    }
    let token = session_cookie(&answer).unwrap();
    let signed: SignedIn = answer.json().await.unwrap();
    assert_eq!(signed.username, "me");
    assert_eq!(signed.recovery_codes.len(), 10);
    assert!(signed.csrf.is_some() && signed.token.is_none());

    let browser = Auth::Cookie { token, csrf: signed.csrf };
    let status: NexusStatus = nexus.get("/api/v1/status", &browser).await;
    assert!(status.ready && status.signed_in);

    // Once only.
    let again = SetupStart { code, username: "you".into(), password: PASSWORD.into() };
    nexus.post_err("/api/v1/setup", &again, &Auth::None, StatusCode::FORBIDDEN).await;

    let audit: Vec<AuditEntry> = nexus.get("/api/v1/audit", &browser).await;
    assert!(audit.iter().any(|e| e.what == "setup"));
}

#[tokio::test]
async fn login_needs_password_and_a_fresh_code() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;

    let code = account.auth.code().await;
    let wrong = Login { username: "me".into(), password: "not the password".into(), totp: code.clone(), device: phone() };
    nexus.post_err("/api/v1/login", &wrong, &Auth::None, StatusCode::UNAUTHORIZED).await;

    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code.clone(), device: phone() };
    let signed: SignedIn = nexus.post("/api/v1/login", &login, &Auth::None).await;
    let token = signed.token.expect("a phone gets a token");
    assert!(signed.csrf.is_none());

    // The same code twice is refused.
    nexus.post_err("/api/v1/login", &login, &Auth::None, StatusCode::UNAUTHORIZED).await;

    // A recovery code works once.
    let recovery = Login { totp: account.recovery[0].to_lowercase(), ..login.clone() };
    let _: SignedIn = nexus.post("/api/v1/login", &recovery, &Auth::None).await;
    nexus.post_err("/api/v1/login", &recovery, &Auth::None, StatusCode::UNAUTHORIZED).await;

    // The phone's token signs in; no CSRF token needed with it.
    let phone = Auth::Bearer(token);
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &phone).await;
    assert_eq!(devices.iter().filter(|d| d.kind == DeviceKind::Phone).count(), 2);
    assert!(devices.iter().any(|d| d.current && d.kind == DeviceKind::Phone));
    let me = devices.iter().find(|d| d.current).unwrap();
    let _: serde_json::Value =
        nexus.post("/api/v1/devices/rename", &RenameDevice { device: me.id.clone(), name: "My phone".into() }, &phone).await;

    let bogus = Auth::Bearer("not-a-token".into());
    assert_eq!(nexus.get_raw("/api/v1/devices", &bogus).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(nexus.get_raw("/api/v1/devices", &Auth::None).await.status(), StatusCode::UNAUTHORIZED);

    let audit: Vec<AuditEntry> = nexus.get("/api/v1/audit", &phone).await;
    assert!(audit.iter().any(|e| e.what == "login-failed"));
    assert!(audit.iter().any(|e| e.what == "login" && e.detail == "with a recovery code"));
}

#[tokio::test]
async fn failed_logins_lock_out() {
    let nexus = start_with(|c| c.login.max_failures = 3).await;
    let mut account = set_up(&nexus).await;
    let wrong = Login { username: "me".into(), password: "wrong password!".into(), totp: "123456".into(), device: phone() };
    for _ in 0..3 {
        nexus.post_err("/api/v1/login", &wrong, &Auth::None, StatusCode::UNAUTHORIZED).await;
    }
    let right = Login { username: "me".into(), password: PASSWORD.into(), totp: account.auth.code().await, device: phone() };
    let err = nexus.post_err("/api/v1/login", &right, &Auth::None, StatusCode::TOO_MANY_REQUESTS).await;
    assert_eq!(err.code, ErrorCode::RateLimited);
}

#[tokio::test]
async fn cookie_sessions_need_the_csrf_token_for_changes() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let Auth::Cookie { token, csrf } = account.browser.clone() else { unreachable!() };
    let without = Auth::Cookie { token: token.clone(), csrf: None };
    let wrong = Auth::Cookie { token: token.clone(), csrf: Some("x".into()) };

    // Reading needs only the cookie.
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &without).await;
    let me = devices.iter().find(|d| d.current).unwrap().id.clone();

    let rename = RenameDevice { device: me, name: "Laptop".into() };
    let err = nexus.post_err("/api/v1/devices/rename", &rename, &without, StatusCode::FORBIDDEN).await;
    assert_eq!(err.code, ErrorCode::Forbidden);
    nexus.post_err("/api/v1/devices/rename", &rename, &wrong, StatusCode::FORBIDDEN).await;
    let _: serde_json::Value = nexus.post("/api/v1/devices/rename", &rename, &account.browser).await;

    // A new password ends the other sessions but this one.
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let change = ChangePassword { old: PASSWORD.into(), new: "another long password".into(), totp: "000000".into() };
    nexus.post_err("/api/v1/password", &change, &account.browser, StatusCode::FORBIDDEN).await;
    let change = ChangePassword { totp: account.auth.code().await, ..change };
    let _: serde_json::Value = nexus.post("/api/v1/password", &change, &account.browser).await;
    assert_eq!(nexus.get_raw("/api/v1/devices", &phone).await.status(), StatusCode::UNAUTHORIZED);
    let _: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;

    // Signing out ends the session.
    let answer = nexus.post_raw("/api/v1/logout", &serde_json::json!({}), &account.browser).await;
    assert_eq!(answer.status(), StatusCode::OK);
    assert!(answer.headers()["set-cookie"].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(nexus.get_raw("/api/v1/devices", &account.browser).await.status(), StatusCode::UNAUTHORIZED);
    let _ = csrf;
}

#[tokio::test]
async fn answers_carry_security_headers() {
    let nexus = start().await;
    let answer = nexus.get_raw("/api/v1/status", &Auth::None).await;
    let headers = answer.headers();
    assert!(headers["content-security-policy"].to_str().unwrap().contains("default-src 'self'"));
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["cache-control"], "no-store");
    let missing = nexus.get_raw("/api/v1/nothing", &Auth::None).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_web_app_falls_back_to_its_index() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html>shed</html>").unwrap();
    std::fs::write(web.path().join("app.js"), "1").unwrap();
    let dir = web.path().to_path_buf();
    let nexus = start_with(|c| c.web_dir = Some(dir)).await;
    let page = nexus.get_raw("/setup?code=x", &Auth::None).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.headers().contains_key("content-security-policy"));
    assert_eq!(page.text().await.unwrap(), "<html>shed</html>");
    assert_eq!(nexus.get_raw("/app.js", &Auth::None).await.text().await.unwrap(), "1");
}

#[tokio::test]
async fn push_subscriptions_are_kept_per_device() {
    let nexus = start().await;
    let account = set_up(&nexus).await;
    let info: PushInfo = nexus.get("/api/v1/push", &account.browser).await;
    assert!(info.vapid_key.is_some());
    assert_eq!(info.prefs.kinds.len(), 4);
    let bad = PushSubscribe { endpoint: "http://x".into(), p256dh: "x".into(), auth: "x".into() };
    nexus.post_err("/api/v1/push/subscribe", &bad, &account.browser, StatusCode::BAD_REQUEST).await;
    let prefs = PushPrefs { kinds: vec![shed_api::wire::NotificationKind::AiWaiting] };
    let _: serde_json::Value = nexus.post("/api/v1/push/prefs", &prefs, &account.browser).await;
    let info: PushInfo = nexus.get("/api/v1/push", &account.browser).await;
    assert_eq!(info.prefs, prefs);
}
