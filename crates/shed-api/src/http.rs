//! The nexus's HTTP API: the account, signing in, devices, pairing a
//! workspace, push. JSON bodies; errors are an `ApiError` with an HTTP status
//! that matches its code.
//!
//! | Route | Body → answer | Who |
//! |---|---|---|
//! | `GET /api/v1/status` | → `NexusStatus` | anyone |
//! | `POST /api/v1/setup` | `SetupStart` → `SetupTotp` | anyone with the setup code (from the setup link, `…/setup#code=…`, which the nexus writes to `setup-link.txt` in its data folder until the account exists) |
//! | `POST /api/v1/setup/confirm` | `SetupConfirm` → `SignedIn` | same |
//! | `POST /api/v1/login` | `Login` → `SignedIn` (a browser also gets the session cookie) | anyone (rate limited) |
//! | `GET /api/v1/session` | → `SignedIn` (no token, no recovery codes; a browser gets its session's current `csrf`) | signed in |
//! | `POST /api/v1/logout` | → `{}` | signed in |
//! | `GET /api/v1/devices` | → `Vec<Device>` | signed in |
//! | `POST /api/v1/devices/revoke` | `Revoke` → `{}` | signed in, with a fresh TOTP |
//! | `POST /api/v1/devices/rename` | `RenameDevice` → `{}` | signed in |
//! | `POST /api/v1/password` | `ChangePassword` → `{}` | signed in, with TOTP |
//! | `POST /api/v1/pair/start` | `PairStart` → `PairStarted` | a workspace (`shed nexus join`) |
//! | `POST /api/v1/pair/poll` | `PairPoll` → `PairState` | that workspace |
//! | `GET /api/v1/pair/pending` | → `Vec<PendingPairing>` (informational: no codes) | signed in |
//! | `POST /api/v1/pair/approve` | `PairApprove` (the code as typed) → `{}` | signed in, with TOTP |
//! | `POST /api/v1/pair/reject` | `PairReject` (its id, or the code) → `{}` | signed in |
//! | `GET /api/v1/push` | → `PushInfo` | signed in |
//! | `POST /api/v1/push/subscribe` | `PushSubscribe` → `{}` | signed in (this device) |
//! | `POST /api/v1/push/unsubscribe` | → `{}` (this device gets no more pushes; its choices stay) | signed in (this device) |
//! | `POST /api/v1/push/prefs` | `PushPrefs` → `{}` | signed in (this device) |
//! | `GET /api/v1/audit` | → `Vec<AuditEntry>` | signed in |
//!
//! A device token goes in `Authorization: Bearer …`; a browser uses the
//! session cookie (and `X-Shed-Csrf` with the token from `SignedIn` on
//! changes). A browser that lost the token (a reload) gets it again from
//! `GET /api/v1/session`.
//!
//! Pushes (Web Push to a browser, UnifiedPush to the Android app) carry a
//! `PushPayload` as JSON, encrypted for the device. With a ntfy topic
//! (`PushInfo::ntfy_topic`), phones get plain ntfy messages on it instead of
//! UnifiedPush. Nothing is pushed while a connector is in view: it gets the
//! notification live (`wire::ServerFrame::Notification`).

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

use crate::wire::NotificationKind;

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexusStatus {
    pub version: String,
    pub api: u32,
    /// The account is set up (otherwise the web app shows the setup).
    pub ready: bool,
    /// Whoever asks is signed in.
    pub signed_in: bool,
}

/// The first step of setting up the account: the setup code (the setup
/// link's `#code=` fragment), a user name and a password.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStart {
    pub code: String,
    pub username: String,
    pub password: String,
}

/// The TOTP secret to add to an authenticator app.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupTotp {
    /// Base32, to type in.
    pub secret: String,
    /// `otpauth://totp/…`, to show as a QR code.
    pub url: String,
}

/// The second step: a code from the authenticator, proving it was added.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupConfirm {
    pub code: String,
    pub totp: String,
    pub device: DeviceHello,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Login {
    pub username: String,
    pub password: String,
    /// A code from the authenticator, or a recovery code.
    pub totp: String,
    pub device: DeviceHello,
}

/// The device signing in.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHello {
    pub kind: DeviceKind,
    /// "Firefox on Linux", "Pixel 8".
    pub name: String,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    /// A browser: gets a session cookie.
    Browser,
    /// The Android app: gets a device token.
    Phone,
    /// A Shed workspace (paired, not signed in).
    Workspace,
    /// Shed's desktop app signed in to show other workspaces (`shed nexus
    /// signin`): gets a device token, like a phone, and may open tunnels
    /// to workspaces that allow desktop clients.
    Desktop,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedIn {
    pub username: String,
    /// This device's id.
    pub device: String,
    /// For a phone or a desktop: its device token, to keep and send as a
    /// bearer token.
    #[serde(default)]
    pub token: Option<String>,
    /// For a browser: the token to send in `X-Shed-Csrf` with changes.
    #[serde(default)]
    pub csrf: Option<String>,
    /// Single-use codes to sign in without the authenticator; only given at
    /// setup and when new ones are made.
    #[serde(default)]
    pub recovery_codes: Vec<String>,
    /// For a desktop or a phone: when its token stops working (Unix
    /// seconds) at the latest, unless it signs in again before. A phone's
    /// may end sooner, after the nexus's `phone_idle_days` without use.
    #[serde(default)]
    pub expires: Option<u32>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub kind: DeviceKind,
    pub name: String,
    /// Unix seconds.
    pub created: u32,
    pub seen: u32,
    /// Where it was last seen from: the address, and its country if known.
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub country: Option<String>,
    /// The device asking.
    #[serde(default)]
    pub current: bool,
    /// Connected now.
    #[serde(default)]
    pub online: bool,
    /// When its sign-in ends (Unix seconds), for a phone or a desktop: a
    /// desktop's some days after signing in; a phone's after some days
    /// unused, or at the latest some days after signing in (using it moves
    /// the first). None: until revoked (a workspace), or for a browser.
    #[serde(default)]
    pub expires: Option<u32>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revoke {
    pub device: String,
    pub totp: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameDevice {
    pub device: String,
    pub name: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePassword {
    pub old: String,
    pub new: String,
    pub totp: String,
}

/// A workspace asks to be paired.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    /// The name it would like (its host name).
    pub name: String,
    pub version: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStarted {
    /// What the person types in the web app to approve it: `K7QF-2M9D`.
    pub code: String,
    /// What the workspace polls with; secret.
    pub poll: String,
    /// Unix seconds; the request is gone after.
    pub expires: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairPoll {
    pub poll: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum PairState {
    Pending,
    /// Approved: the workspace's id and its token, given once.
    Approved(PairApproved),
    Rejected,
    Expired,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairApproved {
    pub workspace: String,
    pub name: String,
    pub token: String,
}

/// A pairing request waiting, for information: which workspace asks and
/// from where. It carries no code: approving takes the code `shed nexus
/// join` printed, typed by the person, so a request someone else made
/// cannot be approved by mistake.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairing {
    /// Always empty (kept for older apps); see `PairApprove::code`.
    pub code: String,
    /// Names it to reject it (`PairReject::id`).
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub version: String,
    pub address: String,
    #[serde(default)]
    pub country: Option<String>,
    pub expires: u32,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairApprove {
    /// The code `shed nexus join` printed on the workspace, as the person
    /// typed it (case, spaces and dashes do not matter).
    pub code: String,
    /// The name to give it (may differ from the one it asked for).
    pub name: String,
    pub totp: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairReject {
    /// The request's code, if known; else empty and `id` set.
    #[serde(default)]
    pub code: String,
    /// The request's id from `PendingPairing`.
    #[serde(default)]
    pub id: String,
}

/// How this nexus pushes.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushInfo {
    /// The VAPID public key for Web Push (base64url), if Web Push is on.
    #[serde(default)]
    pub vapid_key: Option<String>,
    /// This device's choices.
    pub prefs: PushPrefs,
    /// This device has a subscription (`push/subscribe`), so pushes reach
    /// it. False once it unsubscribed, or after the push service said the
    /// subscription is gone.
    #[serde(default)]
    pub subscribed: bool,
    /// The ntfy topic the nexus publishes to when no app is in view
    /// (`https://ntfy.example.com/alerts-shed`), if it has one: phones
    /// subscribe to it in the ntfy app, and get no UnifiedPush then.
    #[serde(default)]
    pub ntfy_topic: Option<String>,
}

/// What a push carries, once decrypted: a workspace's `Notification` with
/// the workspace it came from, added by the nexus. A connector opens
/// `/w/{workspace}/b/{bench}/t/{tab}` from it (as far as those are given);
/// a `Security` one, from the nexus itself, has no workspace (empty) and
/// opens the Devices (`/devices`).
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushPayload {
    pub kind: NotificationKind,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// The workspace's id (`WorkspaceInfo::id`).
    pub workspace: String,
    /// Its name at the nexus.
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default)]
    pub bench: Option<String>,
    #[serde(default)]
    pub tab: Option<String>,
}

/// Where to push to this device: a Web Push subscription (browser), or a
/// UnifiedPush endpoint (Android, through the distributor, such as ntfy).
/// Both are Web Push style: an endpoint and the keys to encrypt for.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushSubscribe {
    pub endpoint: String,
    /// The receiver's public key (base64url, P-256).
    pub p256dh: String,
    /// The shared auth secret (base64url).
    pub auth: String,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushPrefs {
    /// The kinds this device wants.
    pub kinds: Vec<NotificationKind>,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub at: u32,
    /// `login`, `login-failed`, `pair`, `revoke`, `password`, `tunnel-open`…
    pub what: String,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub detail: String,
}
