//! The envelope as the nexus reads it: `shed_api::wire`'s frames, with the
//! workspace's own payloads (`call`, `topic`, `event`, `state`, `ok`) kept as
//! raw JSON. The nexus routes them without reading them, so a call, topic or
//! event it does not know still goes through, byte for byte.
//!
//! Frames are parsed field by field (`parse`) rather than with serde's
//! adjacently tagged enums, which buffer their content when `content` comes
//! before `type` and cannot keep raw JSON then.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use shed_api::ApiError;
use shed_api::wire::{
    LinkTunnelOpen,
    ClientHello, Ended, LinkHello, LinkWelcome, Notification, NotificationFrame, TunnelClosed, TunnelId, TunnelOpen, Unsubscribe,
    WorkspaceInfo,
};

pub type Raw = Box<RawValue>;

/// `ClientFrame`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "content")]
pub enum RawClientFrame {
    Hello(ClientHello),
    Request(RawRequest),
    Subscribe(RawSubscribe),
    Unsubscribe(Unsubscribe),
    Visible(bool),
    TunnelOpen(TunnelOpen),
    TunnelClose(TunnelId),
}

/// `ServerFrame`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "content")]
pub enum RawServerFrame {
    Welcome(RawWelcome),
    Response(RawResponse),
    Event(RawEventFrame),
    Ended(Ended),
    Workspaces(Vec<WorkspaceInfo>),
    State(RawStateFrame),
    Goodbye(ApiError),
    Notification(NotificationFrame),
    TunnelOpened(TunnelId),
    TunnelClosed(TunnelClosed),
}

/// `LinkFrame`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "content")]
pub enum RawLinkFrame {
    Hello(LinkHello),
    State(Raw),
    Response(RawResponse),
    Event(RawEventFrame),
    Ended(Ended),
    Notify(Notification),
    TunnelOpened(TunnelId),
    TunnelClosed(TunnelClosed),
}

/// `NexusFrame`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "content")]
pub enum RawNexusFrame {
    Welcome(LinkWelcome),
    Request(RawRequest),
    Subscribe(RawSubscribe),
    Unsubscribe(Unsubscribe),
    Goodbye(ApiError),
    TunnelOpen(LinkTunnelOpen),
    TunnelClose(TunnelId),
}

/// `Request`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawRequest {
    pub id: u32,
    #[serde(default)]
    pub workspace: String,
    pub call: Raw,
}

/// `Subscribe`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSubscribe {
    pub id: u32,
    #[serde(default)]
    pub workspace: String,
    pub topic: Raw,
}

/// `Response`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawResponse {
    pub id: u32,
    #[serde(default)]
    pub ok: Option<Raw>,
    #[serde(default)]
    pub error: Option<ApiError>,
}

/// `EventFrame`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawEventFrame {
    pub sub: u32,
    pub event: Raw,
}

/// `StateFrame`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawStateFrame {
    pub workspace: String,
    pub state: Raw,
}

/// `Welcome`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawWelcome {
    pub api: u32,
    pub nexus: String,
    pub workspaces: Vec<WorkspaceInfo>,
    pub states: Vec<RawStateFrame>,
}

/// `{"type": …, "content": …}`, in either order.
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(borrow, default)]
    content: Option<&'a RawValue>,
}

fn content<T: DeserializeOwned>(content: Option<&RawValue>) -> serde_json::Result<T> {
    match content {
        Some(raw) => serde_json::from_str(raw.get()),
        None => serde_json::from_str("null"),
    }
}

fn unknown<T>(kind: &str) -> serde_json::Result<T> {
    Err(serde::de::Error::custom(format!("unknown frame type {kind:?}")))
}

impl RawClientFrame {
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        let env: Envelope<'_> = serde_json::from_str(text)?;
        Ok(match env.kind {
            "Hello" => Self::Hello(content(env.content)?),
            "Request" => Self::Request(content(env.content)?),
            "Subscribe" => Self::Subscribe(content(env.content)?),
            "Unsubscribe" => Self::Unsubscribe(content(env.content)?),
            "Visible" => Self::Visible(content(env.content)?),
            "TunnelOpen" => Self::TunnelOpen(content(env.content)?),
            "TunnelClose" => Self::TunnelClose(content(env.content)?),
            other => return unknown(other),
        })
    }
}

impl RawServerFrame {
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        let env: Envelope<'_> = serde_json::from_str(text)?;
        Ok(match env.kind {
            "Welcome" => Self::Welcome(content(env.content)?),
            "Response" => Self::Response(content(env.content)?),
            "Event" => Self::Event(content(env.content)?),
            "Ended" => Self::Ended(content(env.content)?),
            "Workspaces" => Self::Workspaces(content(env.content)?),
            "State" => Self::State(content(env.content)?),
            "Goodbye" => Self::Goodbye(content(env.content)?),
            "Notification" => Self::Notification(content(env.content)?),
            "TunnelOpened" => Self::TunnelOpened(content(env.content)?),
            "TunnelClosed" => Self::TunnelClosed(content(env.content)?),
            other => return unknown(other),
        })
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("frames always serialize")
    }
}

impl RawLinkFrame {
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        let env: Envelope<'_> = serde_json::from_str(text)?;
        Ok(match env.kind {
            "Hello" => Self::Hello(content(env.content)?),
            "State" => Self::State(content(env.content)?),
            "Response" => Self::Response(content(env.content)?),
            "Event" => Self::Event(content(env.content)?),
            "Ended" => Self::Ended(content(env.content)?),
            "Notify" => Self::Notify(content(env.content)?),
            "TunnelOpened" => Self::TunnelOpened(content(env.content)?),
            "TunnelClosed" => Self::TunnelClosed(content(env.content)?),
            other => return unknown(other),
        })
    }
}

impl RawNexusFrame {
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        let env: Envelope<'_> = serde_json::from_str(text)?;
        Ok(match env.kind {
            "Welcome" => Self::Welcome(content(env.content)?),
            "Request" => Self::Request(content(env.content)?),
            "Subscribe" => Self::Subscribe(content(env.content)?),
            "Unsubscribe" => Self::Unsubscribe(content(env.content)?),
            "Goodbye" => Self::Goodbye(content(env.content)?),
            "TunnelOpen" => Self::TunnelOpen(content(env.content)?),
            "TunnelClose" => Self::TunnelClose(content(env.content)?),
            other => return unknown(other),
        })
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("frames always serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shed_api::calls::{BenchOnly, Call, NewBench, Reply};
    use shed_api::state::{Bench, Tab, TabKind, WorkspaceState};
    use shed_api::topics::{Event, ScreenTopic, Topic};
    use shed_api::wire::*;
    use shed_api::{API_VERSION, ErrorCode};

    fn state() -> WorkspaceState {
        WorkspaceState {
            host: "box".into(),
            benches: vec![Bench {
                id: "b1".into(),
                name: "Shed \"dev\" ✓".into(),
                color: "#ff8866".into(),
                cwd: "/home/you/shed".into(),
                tabs: vec![Tab { id: "t1".into(), title: "bash".into(), kind: TabKind::Terminal, ..Tab::default() }],
                active_tab: "t1".into(),
                ..Bench::default()
            }],
            closed: vec![],
            limits: None,
            home: "/home/you".into(),
        }
    }

    fn call() -> Call {
        Call::NewBench(NewBench { name: Some("x \u{1F600}\n".into()), cwd: None })
    }

    fn info() -> WorkspaceInfo {
        WorkspaceInfo { id: "w".into(), name: "PC".into(), online: true, seen: 7, policy: Policy::View, version: "0.1.0".into() }
    }

    /// Typed → JSON → raw → JSON gives the same bytes.
    fn same<T: Serialize>(typed: &T, parse: impl Fn(&str) -> serde_json::Result<String>) {
        let text = serde_json::to_string(typed).unwrap();
        assert_eq!(parse(&text).unwrap(), text);
    }

    #[test]
    fn frames_pass_through_raw_byte_for_byte() {
        let clients = [
            ClientFrame::Hello(ClientHello { api: API_VERSION, client: "web 0.1.0".into() }),
            ClientFrame::Request(Request { id: 3, workspace: "w".into(), call: call() }),
            ClientFrame::Request(Request { id: 4, workspace: "w".into(), call: Call::Previews }),
            ClientFrame::Request(Request { id: 5, workspace: "w".into(), call: Call::CloseBench(BenchOnly { bench: "b".into() }) }),
            ClientFrame::Subscribe(Subscribe { id: 9, workspace: "w".into(), topic: Topic::Screen(ScreenTopic { pane: "p".into() }) }),
            ClientFrame::Unsubscribe(Unsubscribe { id: 9 }),
            ClientFrame::Visible(true),
            ClientFrame::TunnelOpen(TunnelOpen { id: 3, workspace: "w".into() }),
            ClientFrame::TunnelClose(TunnelId { id: 3 }),
        ];
        for frame in &clients {
            same(frame, |t| RawClientFrame::parse(t).map(|f| serde_json::to_string(&f).unwrap()));
        }

        let servers = [
            ServerFrame::Welcome(Welcome {
                api: API_VERSION,
                nexus: "0.1.0".into(),
                workspaces: vec![info()],
                states: vec![StateFrame { workspace: "w".into(), state: state() }],
            }),
            ServerFrame::Response(Response::ok(3, Reply::Bench("b1".into()))),
            ServerFrame::Response(Response::ok(4, Reply::Done)),
            ServerFrame::Response(Response::error(5, ApiError::new(ErrorCode::Offline, "gone"))),
            ServerFrame::Event(EventFrame { sub: 9, event: Event::GitChanged }),
            ServerFrame::Ended(Ended { sub: 9, error: None }),
            ServerFrame::Workspaces(vec![info()]),
            ServerFrame::State(StateFrame { workspace: "w".into(), state: state() }),
            ServerFrame::Goodbye(ApiError::new(ErrorCode::Unauthorized, "revoked")),
            ServerFrame::Notification(NotificationFrame {
                workspace: "w".into(),
                notification: Notification {
                    kind: NotificationKind::AiDone,
                    title: "Claude is done".into(),
                    body: "api · claude".into(),
                    bench: Some("b".into()),
                    tab: Some("t".into()),
                    present: false,
                },
            }),
            ServerFrame::TunnelOpened(TunnelId { id: 3 }),
            ServerFrame::TunnelClosed(TunnelClosed { id: 3, error: Some(ApiError::new(ErrorCode::Forbidden, "no")) }),
        ];
        for frame in &servers {
            same(frame, |t| RawServerFrame::parse(t).map(|f| f.to_text()));
        }

        let links = [
            LinkFrame::Hello(LinkHello { api: API_VERSION, version: "0.1.0".into(), policy: Policy::Full }),
            LinkFrame::State(state()),
            LinkFrame::Response(Response::ok(1, Reply::Tab("t".into()))),
            LinkFrame::Event(EventFrame { sub: 2, event: Event::GitChanged }),
            LinkFrame::Ended(Ended { sub: 2, error: Some(ApiError::new(ErrorCode::NotFound, "tab closed")) }),
            LinkFrame::Notify(Notification {
                kind: NotificationKind::AiWaiting,
                title: "Claude waits".into(),
                body: String::new(),
                bench: Some("b".into()),
                tab: None,
                present: true,
            }),
            LinkFrame::TunnelOpened(TunnelId { id: 4 }),
            LinkFrame::TunnelClosed(TunnelClosed { id: 4, error: None }),
        ];
        for frame in &links {
            same(frame, |t| RawLinkFrame::parse(t).map(|f| serde_json::to_string(&f).unwrap()));
        }

        let nexus = [
            NexusFrame::Welcome(LinkWelcome { api: API_VERSION, nexus: "0.1.0".into(), workspace: "w".into(), name: "PC".into() }),
            NexusFrame::Request(Request { id: 1, workspace: String::new(), call: call() }),
            NexusFrame::Subscribe(Subscribe { id: 2, workspace: String::new(), topic: Topic::Screen(ScreenTopic { pane: "p".into() }) }),
            NexusFrame::Unsubscribe(Unsubscribe { id: 2 }),
            NexusFrame::Goodbye(ApiError::new(ErrorCode::Unsupported, "old")),
            NexusFrame::TunnelOpen(LinkTunnelOpen { id: 5, device: Some(TunnelDevice { id: "d".into(), name: "Shed on laptop".into() }) }),
            NexusFrame::TunnelOpen(LinkTunnelOpen { id: 6, device: None }),
            NexusFrame::TunnelClose(TunnelId { id: 5 }),
        ];
        for frame in &nexus {
            same(frame, |t| RawNexusFrame::parse(t).map(|f| f.to_text()));
        }
    }

    #[test]
    fn unknown_calls_still_relay() {
        let text = r#"{"content":{"id":1,"workspace":"w","call":{"type":"FromTheFuture","content":{"x":[1,2]}}},"type":"Request"}"#;
        let RawClientFrame::Request(req) = RawClientFrame::parse(text).unwrap() else { panic!() };
        assert_eq!(req.call.get(), r#"{"type":"FromTheFuture","content":{"x":[1,2]}}"#);
        assert!(RawClientFrame::parse(r#"{"type":"Nope","content":1}"#).is_err());
    }
}
