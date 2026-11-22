//! The envelope: frames on the nexus's two WebSockets.
//!
//! - `/api`: a connector (signed in: a session cookie or a device token)
//!   sends `ClientFrame`s and gets `ServerFrame`s.
//! - `/link`: a workspace's link (a workspace token, `Authorization: Bearer`)
//!   sends `LinkFrame`s and gets `NexusFrame`s.
//!
//! The nexus routes `Request`s and `Subscribe`s to the workspace they name,
//! under ids of its own, and passes `Response`s and `Event`s back to whoever
//! asked. It reads only this envelope: `call`, `topic`, `event` and `state`
//! pass through it untouched (see `RawCall` and friends on the nexus side).
//! Account, device and pairing work is plain HTTP (`http`), not frames.
//!
//! Every frame is one JSON text message, except a tunnel's data.
//!
//! **Tunnels** (desktop connectors only): Shed's desktop app showing another
//! workspace speaks that workspace's keeper protocol, byte for byte, through
//! the nexus. It opens a tunnel (`ClientFrame::TunnelOpen`, answered
//! `TunnelOpened` or `TunnelClosed`); the nexus asks the link
//! (`NexusFrame::TunnelOpen`, under an id of its own) and the link connects
//! to its keeper's socket. Data goes both ways as WebSocket **binary**
//! messages: a big-endian `u32` tunnel id, then the bytes (`tunnel_data`,
//! `split_tunnel_data`). Either side ends it with `TunnelClose`/`TunnelClosed`;
//! the nexus closes a tunnel when either connection goes, when the device is
//! revoked, and when the workspace goes offline. Browsers and phones may not
//! open tunnels (`Forbidden`); a link refuses them unless its `shed.toml` says
//! `[nexus] desktop_clients = true` (and control is `full`).

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

use crate::ApiError;
use crate::calls::{Call, Reply};
use crate::state::WorkspaceState;
use crate::topics::{Event, Topic};

// ---------------------------------------------------------------- connector

/// From a connector to the nexus.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum ClientFrame {
    /// First frame: the API version the connector speaks.
    Hello(ClientHello),
    Request(Request),
    Subscribe(Subscribe),
    Unsubscribe(Unsubscribe),
    /// Whether the connector is in view (a browser tab shown and focused,
    /// the Android app in the foreground). Sent after `Hello` and on every
    /// change; a connection that never sends it counts as not in view. While
    /// any connector is in view, notifications come to all of them live
    /// (`ServerFrame::Notification`) and nothing is pushed.
    Visible(bool),
    /// Opens a tunnel to a workspace's keeper (desktop connectors only).
    TunnelOpen(TunnelOpen),
    /// Closes one of this connector's tunnels.
    TunnelClose(TunnelId),
}

/// Opens a tunnel to `workspace`'s keeper under `id`.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelOpen {
    /// Chosen by the sender, unique among its open tunnels.
    pub id: u32,
    pub workspace: String,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelId {
    pub id: u32,
}

/// A tunnel ended (or never opened): `error` says why, if it was not asked for.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelClosed {
    pub id: u32,
    #[serde(default)]
    pub error: Option<ApiError>,
}

/// A tunnel's data as a binary WebSocket message: the id (big-endian), then
/// the bytes.
pub fn tunnel_data(id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// The tunnel id and bytes of a binary message; `None` if it is too short.
pub fn split_tunnel_data(message: &[u8]) -> Option<(u32, &[u8])> {
    let (id, bytes) = message.split_first_chunk::<4>()?;
    Some((u32::from_be_bytes(*id), bytes))
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHello {
    pub api: u32,
    /// What the connector is, for logs: `web 0.1.0`, `android 0.1.0`.
    #[serde(default)]
    pub client: String,
}

/// A call on a workspace. Answered by one `Response` with the same id.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// Chosen by the sender, unique among its requests in flight.
    pub id: u32,
    /// The workspace's id (from `WorkspaceInfo`); ignored on `/link`.
    #[serde(default)]
    pub workspace: String,
    pub call: Call,
}

/// Starts a stream of events about something in a workspace. The first event
/// is the whole of it (a transcript, a screen), the next ones its changes.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscribe {
    /// Chosen by the sender; events carry it, `Unsubscribe` names it.
    pub id: u32,
    #[serde(default)]
    pub workspace: String,
    pub topic: Topic,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unsubscribe {
    pub id: u32,
}

/// From the nexus to a connector.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum ServerFrame {
    /// Answers `Hello`: the workspaces, each with its last known state.
    Welcome(Welcome),
    Response(Response),
    Event(EventFrame),
    /// A subscription ended without being asked to (the workspace went
    /// offline, the tab closed); subscribe again to resume.
    Ended(Ended),
    /// A workspace came, went, or was renamed or removed: the whole list.
    Workspaces(Vec<WorkspaceInfo>),
    /// A workspace's state changed: all of it (it is small).
    State(StateFrame),
    /// The connection is being closed: signed out, revoked, or the API
    /// version is not accepted. Not to be retried as is.
    Goodbye(ApiError),
    /// A notification, live: sent to every connector while one is in view
    /// (`ClientFrame::Visible`), instead of a push. The app shows it itself.
    Notification(NotificationFrame),
    /// A tunnel this connector asked for is open: data may flow.
    TunnelOpened(TunnelId),
    /// A tunnel ended, or was refused.
    TunnelClosed(TunnelClosed),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationFrame {
    /// The workspace's id (`WorkspaceInfo::id`).
    pub workspace: String,
    pub notification: Notification,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub api: u32,
    /// The nexus's version, `0.1.0`.
    pub nexus: String,
    pub workspaces: Vec<WorkspaceInfo>,
    /// The last state of each workspace that has one (offline ones too).
    pub states: Vec<StateFrame>,
}

/// A workspace as the nexus knows it.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub id: String,
    /// Its name, given at pairing (the machine's host name by default).
    pub name: String,
    pub online: bool,
    /// When it was last connected (Unix seconds), or connected since.
    pub seen: u32,
    /// What it lets connectors do.
    pub policy: Policy,
    /// Its Shed version, `0.1.0`.
    #[serde(default)]
    pub version: String,
}

/// What a workspace lets connectors do (`[nexus] control` in `shed.toml`).
#[typeshare]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Policy {
    /// Look only: calls that change something are refused (`Forbidden`).
    View,
    /// Everything.
    #[default]
    Full,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFrame {
    pub workspace: String,
    pub state: WorkspaceState,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: u32,
    /// The answer, when the call worked.
    #[serde(default)]
    pub ok: Option<Reply>,
    /// Why not, when it did not.
    #[serde(default)]
    pub error: Option<ApiError>,
}

impl Response {
    pub fn ok(id: u32, reply: Reply) -> Self {
        Response { id, ok: Some(reply), error: None }
    }

    pub fn error(id: u32, error: ApiError) -> Self {
        Response { id, ok: None, error: Some(error) }
    }
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventFrame {
    /// The subscription's id.
    pub sub: u32,
    pub event: Event,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    pub sub: u32,
    #[serde(default)]
    pub error: Option<ApiError>,
}

// --------------------------------------------------------------------- link

/// From a workspace's link to the nexus.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum LinkFrame {
    /// First frame.
    Hello(LinkHello),
    /// The workspace's state, whole, whenever it changes (at most a few times
    /// a second). The nexus keeps the last one.
    State(WorkspaceState),
    Response(Response),
    Event(EventFrame),
    Ended(Ended),
    /// Something worth a notification.
    Notify(Notification),
    /// A tunnel the nexus asked for is open (connected to the keeper).
    TunnelOpened(TunnelId),
    /// A tunnel ended, or was refused (`Forbidden` when the workspace does
    /// not allow desktop clients).
    TunnelClosed(TunnelClosed),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkHello {
    pub api: u32,
    /// Shed's version on the workspace.
    pub version: String,
    pub policy: Policy,
}

/// From the nexus to a link. Ids are the nexus's own.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum NexusFrame {
    Welcome(LinkWelcome),
    Request(Request),
    Subscribe(Subscribe),
    Unsubscribe(Unsubscribe),
    /// Closing: the workspace was removed, or its API version is not accepted.
    Goodbye(ApiError),
    /// A desktop connector wants a tunnel to the keeper, under this id.
    TunnelOpen(TunnelId),
    /// The tunnel's connector went, or closed it.
    TunnelClose(TunnelId),
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkWelcome {
    pub api: u32,
    pub nexus: String,
    /// The workspace's id and name at the nexus.
    pub workspace: String,
    pub name: String,
}

/// Something that happened in a workspace that the user may want to hear
/// about. While a connector is in view the nexus sends it live to the
/// connectors (`ServerFrame::Notification`); otherwise it pushes it: to a
/// ntfy topic if it has one (`PushInfo::ntfy_topic`), and as Web Push, a
/// `http::PushPayload` (this, with the workspace's id and name added), to
/// the browsers (and, without a topic, the phones) that want its kind.
#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub kind: NotificationKind,
    /// One line: "Claude waits for an answer".
    pub title: String,
    /// More, if any: the bench and tab, the question.
    #[serde(default)]
    pub body: String,
    /// Where it happened, to open it from the notification.
    #[serde(default)]
    pub bench: Option<String>,
    #[serde(default)]
    pub tab: Option<String>,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationKind {
    /// An AI tab waits for a permission or an answer.
    AiWaiting,
    /// An AI tab finished a turn.
    AiDone,
    /// A command that ran long finished.
    CommandDone,
    /// A workspace went offline (sent by the nexus itself).
    WorkspaceOffline,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_data_round_trips() {
        let message = tunnel_data(0x0102_0304, b"hello");
        assert_eq!(&message[..4], &[1, 2, 3, 4]);
        assert_eq!(split_tunnel_data(&message), Some((0x0102_0304, &b"hello"[..])));
        assert_eq!(split_tunnel_data(&tunnel_data(7, b"")), Some((7, &b""[..])));
        assert_eq!(split_tunnel_data(b"abc"), None);
    }
}
