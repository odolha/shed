//! The WebSocket to the nexus: dialled out, with the workspace's token;
//! dialled again after a drop, waiting longer each time (1 s up to a
//! minute). Frames are JSON text, one message each.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Welcomed: frames for the nexus go to `out`, tunnels' data (binary
    /// messages, `[id][bytes]`) to `data`, until `Disconnected`.
    Connected { out: mpsc::UnboundedSender<LinkFrame>, data: mpsc::Sender<Vec<u8>>, welcome: LinkWelcome },
    Frame(NexusFrame),
    /// A tunnel's data from the nexus (counted in the `Backlog` until the
    /// link takes it).
    Data(u32, Vec<u8>),
    /// A tunnel's data would have gone past `DATA_BACKLOG`: dropped, and
    /// the tunnel with it (its later data is dropped too).
    Overflow(u32),
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
    pub backlog: Backlog,
}

/// Tunnel data from the nexus waiting for the link to take it, at most, in
/// bytes: past it, the tunnel that would add more is dropped rather than
/// buffered without end (the nexus allows each connection as much).
pub(crate) const DATA_BACKLOG: usize = 8 * 1024 * 1024;

/// Bytes of tunnel data sent to the link and not taken yet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Backlog(Arc<AtomicUsize>);

impl Backlog {
    /// The link took `n` bytes.
    pub(crate) fn taken(&self, n: usize) {
        self.0.fetch_sub(n, Ordering::AcqRel);
    }
}

/// Lets tunnel data through while the backlog has room.
struct Gate {
    backlog: Backlog,
    limit: usize,
    /// Tunnels dropped for going past it.
    dropped: HashSet<u32>,
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Pass,
    /// Past the limit: the tunnel is dropped (said once).
    Overflow,
    /// A dropped tunnel's data.
    Drop,
}

impl Gate {
    fn new(backlog: Backlog, limit: usize) -> Gate {
        Gate { backlog, limit, dropped: HashSet::new() }
    }

    fn admit(&mut self, id: u32, len: usize) -> Admit {
        if self.dropped.contains(&id) {
            return Admit::Drop;
        }
        // Only this task adds, so the room seen is there.
        if self.backlog.0.load(Ordering::Acquire) + len > self.limit {
            self.dropped.insert(id);
            return Admit::Overflow;
        }
        self.backlog.0.fetch_add(len, Ordering::AcqRel);
        Admit::Pass
    }

    /// The nexus opened tunnel `id` (again): its data counts anew.
    fn opened(&mut self, id: u32) {
        self.dropped.remove(&id);
    }
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

/// Tunnel data waiting for the nexus, at most (pieces of up to 64 KB):
/// past it, tunnels wait before reading more from the keeper.
const DATA_QUEUE: usize = 64;

/// The largest frame the link reads: the nexus's limit (24 MB), and more.
const MAX_MESSAGE: usize = 32 * 1024 * 1024;

/// One connection, from dialling to its end. `Ok` when the link is going away.
async fn session(dial: &Dial, events: &mpsc::UnboundedSender<NexusEvent>) -> Result<(), Ended> {
    let mut request = dial.url.as_str().into_client_request().map_err(|e| lost(format!("bad address: {e}")))?;
    let bearer = HeaderValue::from_str(&format!("Bearer {}", dial.token)).map_err(|_| Ended::Refused(ApiError::new(ErrorCode::Unauthorized, "the token is not valid")))?;
    request.headers_mut().insert("authorization", bearer);
    let connector = tokio_tungstenite::Connector::Rustls(dial.tls.clone());
    // Frames as large as the nexus relays (pictures and files for AI prompts
    // travel inside them; tungstenite's default is 16 MB a frame).
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_frame_size(Some(MAX_MESSAGE)).max_message_size(Some(MAX_MESSAGE));
    let (ws, _) = match tokio_tungstenite::connect_async_tls_with_config(request, Some(config), true, Some(connector)).await {
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
    let (data, mut data_rx) = mpsc::channel::<Vec<u8>>(DATA_QUEUE);
    if events.send(NexusEvent::Connected { out, data, welcome }).is_err() {
        return Ok(());
    }
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let gone = |why: String| Ended::Lost { why, welcomed: true };
    let mut gate = Gate::new(dial.backlog.clone(), DATA_BACKLOG);
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                None => return Err(gone("closed by the nexus".into())),
                Some(Err(e)) => return Err(gone(e.to_string())),
                Some(Ok(Message::Close(frame))) => return Err(gone(format!("closed by the nexus{}", frame.map(|f| format!(": {}", f.reason)).unwrap_or_default()))),
                Some(Ok(Message::Binary(b))) => {
                    let Some((id, bytes)) = shed_api::wire::split_tunnel_data(&b) else { continue };
                    let event = match gate.admit(id, bytes.len()) {
                        Admit::Pass => NexusEvent::Data(id, bytes.to_vec()),
                        Admit::Overflow => NexusEvent::Overflow(id),
                        Admit::Drop => continue,
                    };
                    if events.send(event).is_err() {
                        return Ok(());
                    }
                }
                Some(Ok(m)) => {
                    let event = match parse(m) {
                        Some(Ok(NexusFrame::Goodbye(e))) => return Err(Ended::Refused(e)),
                        Some(Ok(NexusFrame::TunnelOpen(open))) => {
                            gate.opened(open.id);
                            NexusEvent::Frame(NexusFrame::TunnelOpen(open))
                        }
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
            Some(bytes) = data_rx.recv() => {
                if let Err(e) = sink.send(Message::binary(bytes)).await {
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
        Err(_) => {
            // What it says may be private (typed text, file contents): its
            // type and length only.
            let v: Option<serde_json::Value> = serde_json::from_str(&text).ok();
            let kind = v.as_ref().and_then(|v| v.get("type")).and_then(|t| t.as_str()).map(|t| t.chars().take(32).collect::<String>());
            warn!("a frame from the nexus did not read (type {}, {} bytes)", kind.as_deref().unwrap_or("?"), text.len());
            let v = v?;
            let id = v.get("content").and_then(|c| c.get("id")).and_then(|i| i.as_u64()).map(|i| i as u32);
            match v.get("type").and_then(|t| t.as_str()) {
                Some("Request") => Some(Err((id, None))),
                Some("Subscribe") => Some(Err((None, id))),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_data_past_the_backlog_drops_its_tunnel() {
        let backlog = Backlog::default();
        let mut gate = Gate::new(backlog.clone(), 100);
        assert_eq!(gate.admit(1, 60), Admit::Pass);
        assert_eq!(gate.admit(2, 30), Admit::Pass);
        // Tunnel 1 would go past it: dropped, and its data after.
        assert_eq!(gate.admit(1, 20), Admit::Overflow);
        assert_eq!(gate.admit(1, 1), Admit::Drop);
        // Others carry on while there is room.
        assert_eq!(gate.admit(2, 10), Admit::Pass);
        assert_eq!(gate.admit(2, 1), Admit::Overflow);
        // The link takes what came; a tunnel opened again counts anew.
        backlog.taken(100);
        assert_eq!(gate.admit(1, 50), Admit::Drop);
        gate.opened(1);
        assert_eq!(gate.admit(1, 50), Admit::Pass);
        // A single piece larger than the whole backlog never passes.
        gate.opened(3);
        assert_eq!(gate.admit(3, 101), Admit::Overflow);
    }
}
