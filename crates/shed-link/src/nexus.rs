//! The WebSocket to the nexus: dialled out, with the workspace's token;
//! dialled again after a drop, waiting longer each time (1 s up to a
//! minute). Frames are JSON text, one message each.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use shed_api::wire::{LinkFrame, LinkHello, LinkWelcome, NexusFrame};
use shed_api::{ApiError, ErrorCode};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tracing::{debug, info, warn};

/// What the connection tells the link.
#[derive(Debug)]
pub(crate) enum NexusEvent {
    /// Welcomed: frames for the nexus go to `out` until `Disconnected`.
    Connected { out: mpsc::UnboundedSender<LinkFrame>, welcome: LinkWelcome },
    Frame(NexusFrame),
    /// A request or subscription this link does not understand (from a
    /// newer API): answered `Unsupported`.
    Unknown { request: Option<u32>, sub: Option<u32> },
    Disconnected(String),
    /// Not to be tried again: the nexus said goodbye or does not know the token.
    Refused(ApiError),
}

/// How long to wait for the nexus's welcome.
const WELCOME_WAIT: Duration = Duration::from_secs(15);
/// A ping this often keeps proxies from closing a quiet connection.
const PING_EVERY: Duration = Duration::from_secs(30);

pub(crate) struct Dial {
    pub url: String,
    pub token: String,
    pub hello: LinkHello,
    pub tls: Arc<rustls::ClientConfig>,
}

/// Connects, and connects again, until refused or until `events` closes.
pub(crate) async fn run(dial: Dial, events: mpsc::UnboundedSender<NexusEvent>) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match session(&dial, &events).await {
            Ok(()) => return,
            Err(Ended::Refused(e)) => {
                let _ = events.send(NexusEvent::Refused(e));
                return;
            }
            Err(Ended::Lost { why, welcomed }) => {
                if events.send(NexusEvent::Disconnected(why.clone())).is_err() {
                    return;
                }
                if welcomed {
                    backoff = Duration::from_secs(1);
                }
                info!("nexus connection: {why}; again in {}s", backoff.as_secs());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

enum Ended {
    Refused(ApiError),
    Lost { why: String, welcomed: bool },
}

fn lost(why: impl Into<String>) -> Ended {
    Ended::Lost { why: why.into(), welcomed: false }
}

/// One connection, from dialling to its end. `Ok` when the link is going away.
async fn session(dial: &Dial, events: &mpsc::UnboundedSender<NexusEvent>) -> Result<(), Ended> {
    let mut request = dial.url.as_str().into_client_request().map_err(|e| lost(format!("bad address: {e}")))?;
    let bearer = HeaderValue::from_str(&format!("Bearer {}", dial.token)).map_err(|_| Ended::Refused(ApiError::new(ErrorCode::Unauthorized, "the token is not valid")))?;
    request.headers_mut().insert("authorization", bearer);
    let connector = tokio_tungstenite::Connector::Rustls(dial.tls.clone());
    let (ws, _) = match tokio_tungstenite::connect_async_tls_with_config(request, None, true, Some(connector)).await {
        Ok(c) => c,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if matches!(response.status().as_u16(), 401 | 403) => {
            return Err(Ended::Refused(ApiError::new(ErrorCode::Unauthorized, format!("the nexus does not know this workspace ({})", response.status()))));
        }
        Err(e) => return Err(lost(format!("cannot connect: {e}"))),
    };
    let (mut sink, mut stream) = ws.split();
    let hello = serde_json::to_string(&LinkFrame::Hello(dial.hello.clone())).expect("frames serialize");
    sink.send(Message::text(hello)).await.map_err(|e| lost(e.to_string()))?;

    // The first frame is the welcome (or a goodbye).
    let welcome = loop {
        let msg = match tokio::time::timeout(WELCOME_WAIT, stream.next()).await {
            Err(_) => return Err(lost("no welcome from the nexus")),
            Ok(None) => return Err(lost("closed before the welcome")),
            Ok(Some(Err(e))) => return Err(lost(e.to_string())),
            Ok(Some(Ok(m))) => m,
        };
        match parse(msg) {
            Some(Ok(NexusFrame::Welcome(w))) => break w,
            Some(Ok(NexusFrame::Goodbye(e))) => return Err(Ended::Refused(e)),
            Some(other) => debug!("before the welcome: {other:?}"),
            None => {}
        }
    };
    info!(workspace = %welcome.name, nexus = %welcome.nexus, "linked");
    let (out, mut out_rx) = mpsc::unbounded_channel::<LinkFrame>();
    if events.send(NexusEvent::Connected { out, welcome }).is_err() {
        return Ok(());
    }
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let gone = |why: String| Ended::Lost { why, welcomed: true };
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                None => return Err(gone("closed by the nexus".into())),
                Some(Err(e)) => return Err(gone(e.to_string())),
                Some(Ok(Message::Close(frame))) => return Err(gone(format!("closed by the nexus{}", frame.map(|f| format!(": {}", f.reason)).unwrap_or_default()))),
                Some(Ok(m)) => {
                    let event = match parse(m) {
                        Some(Ok(NexusFrame::Goodbye(e))) => return Err(Ended::Refused(e)),
                        Some(Ok(frame)) => NexusEvent::Frame(frame),
                        Some(Err((request, sub))) => NexusEvent::Unknown { request, sub },
                        None => continue,
                    };
                    if events.send(event).is_err() {
                        return Ok(());
                    }
                }
            },
            frame = out_rx.recv() => {
                // The link dropped the sender: it is going away.
                let Some(frame) = frame else {
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                };
                let text = serde_json::to_string(&frame).expect("frames serialize");
                if let Err(e) = sink.send(Message::text(text)).await {
                    return Err(gone(e.to_string()));
                }
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Ping(Vec::new().into())).await {
                    return Err(gone(e.to_string()));
                }
            }
        }
    }
}

/// A request's and a subscription's id, from a frame that did not read.
type Unknown = (Option<u32>, Option<u32>);

/// A frame from the nexus; `None` for pings. One that does not read (a
/// newer API's call or topic) gives the request's or subscription's id, to
/// answer it.
fn parse(msg: Message) -> Option<Result<NexusFrame, Unknown>> {
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) | Message::Close(_) => return None,
    };
    match serde_json::from_str::<NexusFrame>(&text) {
        Ok(f) => Some(Ok(f)),
        Err(e) => {
            warn!("a frame from the nexus did not read ({e}): {}", text.chars().take(200).collect::<String>());
            let v: serde_json::Value = serde_json::from_str(&text).ok()?;
            let id = v.get("content").and_then(|c| c.get("id")).and_then(|i| i.as_u64()).map(|i| i as u32);
            match v.get("type").and_then(|t| t.as_str()) {
                Some("Request") => Some(Err((id, None))),
                Some("Subscribe") => Some(Err((None, id))),
                _ => None,
            }
        }
    }
}
