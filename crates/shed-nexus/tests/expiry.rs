//! Phone sign-ins end: after `phone_idle_days` unused, and `phone_days`
//! after signing in at the latest; the token is then refused with why, and
//! phones signed in with an older nexus get an end from then on. Time is
//! moved by changing the database under the running nexus.

mod common;

use common::*;
use reqwest::StatusCode;
use shed_api::ErrorCode;
use shed_api::http::*;

const DAY: u64 = 86400;

/// Runs `sql` on the nexus's database, beside the nexus.
fn sql(nexus: &Nexus, sql: &str) {
    let db = rusqlite::Connection::open(nexus.dir.path().join("nexus.db")).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    db.execute_batch(sql).unwrap();
}

async fn error_of(nexus: &Nexus, auth: &Auth) -> shed_api::ApiError {
    let answer = nexus.get_raw("/api/v1/devices", auth).await;
    assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    answer.json().await.unwrap()
}

#[tokio::test]
async fn a_phones_sign_in_says_when_it_ends() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let code = account.recovery.pop().unwrap();
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: phone() };
    let signed: SignedIn = nexus.post("/api/v1/login", &login, &Auth::None).await;
    let end = signed.expires.expect("a phone's sign-in ends") as u64;
    assert!(end.abs_diff(unix() + 60 * DAY) < 60, "{end}");
    let desktop = desktop_token(&nexus, &mut account).await;

    // Devices: the phone's ends unused first; the desktop's at its end; a
    // browser's not given.
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    let of = |kind| devices.iter().find(|d| d.kind == kind).unwrap();
    let phone_end = of(DeviceKind::Phone).expires.unwrap() as u64;
    assert!(phone_end.abs_diff(unix() + 7 * DAY) < 120, "{phone_end}");
    let desktop_end = of(DeviceKind::Desktop).expires.unwrap() as u64;
    assert!(desktop_end.abs_diff(unix() + 30 * DAY) < 60, "{desktop_end}");
    assert_eq!(of(DeviceKind::Browser).expires, None);
    let _ = desktop;
}

#[tokio::test]
async fn an_unused_phone_is_signed_out_and_told_why() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let _: Vec<Device> = nexus.get("/api/v1/devices", &phone).await;

    // Last used eight days ago.
    let ago = unix() - 8 * DAY;
    sql(&nexus, &format!("UPDATE devices SET used = {ago}, seen = {ago} WHERE kind = 'phone'"));
    let error = error_of(&nexus, &phone).await;
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(error.message, "signed out: not used for 7 days");
    let Auth::Bearer(token) = &phone else { unreachable!() };
    let refused = connect(&nexus.ws_url("/api"), &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(refused.err(), Some(401));
    // Not listed, and trying again does not bring it back.
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    assert!(devices.iter().all(|d| d.kind != DeviceKind::Phone));
    assert_eq!(error_of(&nexus, &phone).await.message, "signed out: not used for 7 days");
}

#[tokio::test]
async fn using_a_phone_keeps_it_signed_in() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    // Six days unused: still in, and using it starts the days again.
    let ago = unix() - 6 * DAY;
    sql(&nexus, &format!("UPDATE devices SET used = {ago}, seen = {ago} WHERE kind = 'phone'"));
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &phone).await;
    let end = devices.iter().find(|d| d.current).unwrap().expires.unwrap() as u64;
    assert!(end.abs_diff(unix() + 7 * DAY) < 120, "the idle window moved: {end}");
}

#[tokio::test]
async fn a_phone_ends_at_its_end_however_used() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let past = unix() - 1;
    sql(&nexus, &format!("UPDATE devices SET expires = {past} WHERE kind = 'phone'"));
    assert_eq!(error_of(&nexus, &phone).await.message, "signed out: the sign-in is older than 60 days");
}

#[tokio::test]
async fn a_revoked_phone_is_told_so() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let phone = Auth::Bearer(phone_token(&nexus, &mut account).await);
    let devices: Vec<Device> = nexus.get("/api/v1/devices", &account.browser).await;
    let id = devices.iter().find(|d| d.kind == DeviceKind::Phone).unwrap().id.clone();
    let revoke = Revoke { device: id, totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/devices/revoke", &revoke, &account.browser).await;
    assert_eq!(error_of(&nexus, &phone).await.message, "signed out: this device was revoked");
    assert_eq!(error_of(&nexus, &Auth::Bearer("never-a-token".into())).await.message, "not signed in");
}

#[tokio::test]
async fn phones_from_an_older_nexus_get_an_end_from_now() {
    let nexus = start().await;
    let mut account = set_up(&nexus).await;
    let token = phone_token(&nexus, &mut account).await;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    // The same database as an older nexus left it: a phone with no end,
    // last seen a month ago.
    let month = unix() - 30 * DAY;
    sql(&nexus, &format!("UPDATE devices SET expires = NULL, used = NULL, seen = {month} WHERE kind = 'phone'"));
    nexus.server.stop().await;
    for name in ["nexus.db", "nexus.db-wal", "nexus.db-shm"] {
        let from = nexus.dir.path().join(name);
        if from.exists() {
            std::fs::copy(from, data.join(name)).unwrap();
        }
    }
    let again = start_with(|c| c.data_dir = Some(data.clone())).await;
    let phone = Auth::Bearer(token);
    let devices: Vec<Device> = again.get("/api/v1/devices", &phone).await;
    let me = devices.iter().find(|d| d.current).unwrap();
    let end = me.expires.unwrap() as u64;
    assert!(end.abs_diff(unix() + 7 * DAY) < 120, "unused from now on: {end}");
}
