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

    // A browser that lost its token (a reload) asks for it again.
    let session: SignedIn = nexus.get("/api/v1/session", &without).await;
    assert_eq!(session.csrf, csrf);
    assert!(session.token.is_none() && session.recovery_codes.is_empty());
    assert_eq!(session.username, "me");
    assert_eq!(nexus.get_raw("/api/v1/session", &Auth::None).await.status(), StatusCode::UNAUTHORIZED);

    // Reading needs only the cookie.
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &without).await;
    assert_eq!(session.device, devices.iter().find(|d| d.current).unwrap().id);
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
    std::fs::write(web.path().join("main-JLFEZLOM.js"), "2").unwrap();
    let dir = web.path().to_path_buf();
    let nexus = start_with(|c| c.web_dir = Some(dir)).await;
    let page = nexus.get_raw("/setup?code=x", &Auth::None).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.headers().contains_key("content-security-policy"));
    assert_eq!(page.headers()["cache-control"], "no-cache");
    assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(page.text().await.unwrap(), "<html>shed</html>");
    let script = nexus.get_raw("/app.js", &Auth::None).await;
    assert_eq!(script.headers()["cache-control"], "no-cache");
    assert_eq!(script.text().await.unwrap(), "1");
    // Hashed bundles are kept for good, and not sent again while unchanged.
    let bundle = nexus.get_raw("/main-JLFEZLOM.js", &Auth::None).await;
    assert_eq!(bundle.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(bundle.headers()["content-type"], "text/javascript; charset=utf-8");
    let etag = bundle.headers()["etag"].to_str().unwrap().to_string();
    let again = nexus.http.get(format!("{}/main-JLFEZLOM.js", nexus.base)).header("if-none-match", etag).send().await.unwrap();
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
    // Nothing outside the folder.
    let outside = nexus.http.get(format!("{}/%2e%2e/account.rs", nexus.base)).send().await.unwrap();
    assert_eq!(outside.text().await.unwrap(), "<html>shed</html>");
    // A deep link gets the page too, and its `<base href="/">` counts.
    let deep = nexus.get_raw("/w/abc/b/def/t/ghi", &Auth::None).await;
    let csp = deep.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.contains("base-uri 'self'"), "{csp}");
    assert_eq!(deep.text().await.unwrap(), "<html>shed</html>");
}

/// Release builds (and `SHED_NEXUS_WEB=1`) carry the web app; debug builds
/// have none, and this test has nothing to check.
#[tokio::test]
async fn the_built_in_web_app_is_served() {
    if shed_nexus::web::EMBEDDED.is_empty() {
        let nexus = start().await;
        let page = nexus.get_raw("/", &Auth::None).await;
        assert_eq!(page.status(), StatusCode::NOT_FOUND);
        return;
    }
    let nexus = start().await;
    let page = nexus.get_raw("/w/abc/b/def/t/ghi", &Auth::None).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()["cache-control"], "no-cache");
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("base-uri 'self'"));
    let html = page.text().await.unwrap();
    assert!(html.contains("<base href=\"/\">") || html.contains("<base href=\"/\"/>"), "{html}");
    let bundle = shed_nexus::web::EMBEDDED.iter().find(|f| f.path.starts_with("main-")).expect("a main bundle");
    let script = nexus.get_raw(&format!("/{}", bundle.path), &Auth::None).await;
    assert_eq!(script.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(script.headers()["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(script.bytes().await.unwrap().as_ref(), bundle.bytes);
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
    assert!(!info.subscribed);
}

#[tokio::test]
async fn a_device_unsubscribes_and_keeps_its_choices() {
    let nexus = start_with(|c| {
        c.push.allow_private_endpoints = true;
        c.push.allow_http_endpoints = true;
    })
    .await;
    let account = set_up(&nexus).await;
    use base64::Engine;
    use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
    let secret = web_push_native::p256::SecretKey::random(&mut web_push_native::p256::elliptic_curve::rand_core::OsRng);
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    let sub = PushSubscribe { endpoint: "http://127.0.0.1:9/push".into(), p256dh: key, auth: "AAAAAAAAAAAAAAAAAAAAAA".into() };
    let _: serde_json::Value = nexus.post("/api/v1/push/subscribe", &sub, &account.browser).await;
    let prefs = PushPrefs { kinds: vec![shed_api::wire::NotificationKind::AiDone] };
    let _: serde_json::Value = nexus.post("/api/v1/push/prefs", &prefs, &account.browser).await;
    let info: PushInfo = nexus.get("/api/v1/push", &account.browser).await;
    assert!(info.subscribed);
    let _: serde_json::Value = nexus.post("/api/v1/push/unsubscribe", &serde_json::json!({}), &account.browser).await;
    let info: PushInfo = nexus.get("/api/v1/push", &account.browser).await;
    assert!(!info.subscribed);
    assert_eq!(info.prefs, prefs);
}
