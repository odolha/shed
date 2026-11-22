//! Shed's desktop app as a nexus connector: its sign-in (kept in
//! `nexus-client.json`, apart from the workspace's pairing), the nexus's
//! list of workspaces, and a tunnel to another workspace's keeper, over
//! which the desktop speaks the keeper's own protocol (see
//! `shed_api::wire`, "Tunnels").
//!
//! The tunnel runs on a thread of its own with a small tokio runtime; to the
//! rest of the app it is a `KeeperConn` and a channel of `Frame`s, as a
//! local keeper's socket is.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use shed_api::wire::{ClientFrame, ClientHello, ServerFrame, TunnelId, TunnelOpen, split_tunnel_data, tunnel_data};
pub use shed_api::wire::WorkspaceInfo;
use shed_api::{ApiError, ErrorCode};
use shed_model::paths;
use shed_proto::{ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, PROTOCOL_VERSION};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::keeper_conn::{KeeperConn, OTHER_VERSION};
use crate::nexus_tls;

/// The desktop signed in to a nexus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    /// The nexus, `https://nexus.example.com`.
    pub url: String,
    pub username: String,
    /// This desktop's device id there.
    pub device: String,
    /// Its device token (a bearer token).
    pub token: String,
}

impl SignIn {
    pub fn load() -> Option<SignIn> {
        Self::load_from(&paths::nexus_client_path())
    }

    pub fn load_from(path: &Path) -> Option<SignIn> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&paths::nexus_client_path())
    }

    /// Writes it whole, readable by its owner only.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(self).expect("serializes").as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Forgets the sign-in here (the nexus still lists the device until it
    /// is revoked there). Whether there was one.
    pub fn remove() -> std::io::Result<bool> {
        match std::fs::remove_file(paths::nexus_client_path()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// The certificates `shed.toml` says to trust for a nexus (`[nexus] ca_file`).
pub fn configured_ca_file() -> Option<std::path::PathBuf> {
    shed_model::config::Config::load_from(&shed_model::config::Config::path()).ok().and_then(|c| c.nexus.ca_file)
}

/// The largest message read: as the nexus relays.
pub(crate) const MAX_MESSAGE: usize = 32 * 1024 * 1024;
/// Tunnel data goes in pieces of at most this size.
const CHUNK: usize = 256 * 1024;
/// How long connecting, the nexus's welcome and the tunnel may take.
const WAIT: Duration = Duration::from_secs(15);
/// A ping this often keeps proxies from closing a quiet connection.
pub(crate) const PING_EVERY: Duration = Duration::from_secs(30);
/// The tunnel's id (one tunnel per connection).
const TUNNEL: u32 = 1;

pub(crate) type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connects to the nexus's `/api` as this desktop and waits for its welcome.
pub(crate) async fn open_api(url: &str, token: &str, ca_file: Option<&Path>) -> anyhow::Result<(Socket, shed_api::wire::Welcome)> {
    let ws_url = nexus_tls::api_ws_url(url)?;
    let tls = nexus_tls::client_config(ca_file)?;
    let mut request = ws_url.as_str().into_client_request().context("the nexus's address")?;
    let bearer = HeaderValue::from_str(&format!("Bearer {token}")).context("the sign-in's token")?;
    request.headers_mut().insert("authorization", bearer);
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(MAX_MESSAGE))
        .max_message_size(Some(MAX_MESSAGE));
    let connector = tokio_tungstenite::Connector::Rustls(tls);
    let connecting = tokio_tungstenite::connect_async_tls_with_config(request, Some(config), true, Some(connector));
    let (mut ws, _) = match tokio::time::timeout(WAIT, connecting).await {
        Err(_) => bail!("the nexus at {url} did not answer"),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r))) if matches!(r.status().as_u16(), 401 | 403) => {
            bail!("the nexus no longer knows this desktop ({}): sign in again", r.status())
        }
        Ok(Err(e)) => bail!("cannot reach the nexus at {url}: {e}"),
        Ok(Ok(c)) => c,
    };
    let hello = ClientFrame::Hello(ClientHello { api: shed_api::API_VERSION, client: concat!("shed-desktop ", env!("CARGO_PKG_VERSION")).into() });
    ws.send(Message::text(serde_json::to_string(&hello).expect("frames serialize"))).await?;
    let welcome = tokio::time::timeout(WAIT, async {
        loop {
            match next_server_frame(&mut ws).await? {
                ServerFrame::Welcome(w) => return Ok(w),
                ServerFrame::Goodbye(e) => bail!("the nexus refused: {}", e.message),
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("no welcome from the nexus"))??;
    Ok((ws, welcome))
}

/// The next text frame that reads as a `ServerFrame` (others are skipped).
async fn next_server_frame(ws: &mut Socket) -> anyhow::Result<ServerFrame> {
    loop {
        match ws.next().await {
            None | Some(Ok(Message::Close(_))) => bail!("the nexus closed the connection"),
            Some(Err(e)) => bail!("the connection to the nexus failed: {e}"),
            Some(Ok(Message::Text(text))) => {
                if let Ok(frame) = serde_json::from_str::<ServerFrame>(&text) {
                    return Ok(frame);
                }
            }
            Some(Ok(_)) => {}
        }
    }
}

pub(crate) fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().context("starting a runtime")
}

/// The nexus's workspaces, as it lists them to this desktop.
pub fn workspaces(sign_in: &SignIn, ca_file: Option<&Path>) -> anyhow::Result<Vec<WorkspaceInfo>> {
    runtime()?.block_on(async {
        let (mut ws, welcome) = open_api(&sign_in.url, &sign_in.token, ca_file).await?;
        let _ = ws.close(None).await;
        Ok(welcome.workspaces)
    })
}

/// Why a tunnel did not open, for people.
fn refused(workspace: &str, error: Option<ApiError>) -> anyhow::Error {
    let Some(error) = error else { return anyhow::anyhow!("{workspace} closed the connection") };
    match error.code {
        ErrorCode::Offline => anyhow::anyhow!("{workspace} is offline"),
        ErrorCode::NotFound => anyhow::anyhow!("the nexus has no workspace {workspace}"),
        ErrorCode::Forbidden => anyhow::anyhow!(
            "{workspace} does not let desktops in: set desktop_clients = true under [nexus] in its shed.toml (with control = \"full\"). {}",
            error.message
        ),
        _ => anyhow::anyhow!("{workspace}: {}", error.message),
    }
}

/// Connects to `workspace`'s keeper through the nexus at `url`: opens the
/// tunnel, says `Hello` as `kind` and waits for the keeper's welcome. A
/// keeper of another protocol version is refused here, before anything is
/// shown. Frames from the keeper come on the channel, which closes when the
/// tunnel ends (an `Error` frame first says why, if it was not asked for).
pub fn connect_tunnel(
    url: &str,
    token: &str,
    workspace: &str,
    ca_file: Option<&Path>,
    kind: ClientKind,
) -> anyhow::Result<(KeeperConn, async_channel::Receiver<Frame>)> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<()>>(1);
    let (in_tx, in_rx) = async_channel::unbounded();
    let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>(64);
    let close = Arc::new(Notify::new());
    let (url, token, workspace, ca_file) = (url.to_string(), token.to_string(), workspace.to_string(), ca_file.map(Path::to_path_buf));
    {
        let close = close.clone();
        std::thread::Builder::new().name("nexus-tunnel".into()).spawn(move || {
            let rt = match runtime() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(tunnel(Dial { url, token, workspace, ca_file, kind }, ready_tx, in_tx, bytes_rx, close));
        })?;
    }
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => bail!("the connection to the nexus stopped"),
    }

    // Frames to send: encoded on a thread of their own, handed to the tunnel.
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Frame>();
    std::thread::Builder::new().name("tunnel-write".into()).spawn(move || {
        while let Ok(frame) = out_rx.recv() {
            let mut buf = Vec::new();
            frame.encode_into(&mut buf);
            while buf.len() < CHUNK
                && let Ok(more) = out_rx.try_recv()
            {
                more.encode_into(&mut buf);
            }
            if bytes_tx.blocking_send(buf).is_err() {
                break;
            }
        }
    })?;
    let closer = move || close.notify_one();
    Ok((KeeperConn::with_closer(out_tx, None, Arc::new(closer)), in_rx))
}

struct Dial {
    url: String,
    token: String,
    workspace: String,
    ca_file: Option<std::path::PathBuf>,
    kind: ClientKind,
}

/// Opens the tunnel, then carries bytes both ways until either side ends.
async fn tunnel(
    dial: Dial,
    ready: std::sync::mpsc::SyncSender<anyhow::Result<()>>,
    frames: async_channel::Sender<Frame>,
    mut out: mpsc::Receiver<Vec<u8>>,
    close: Arc<Notify>,
) {
    let mut decoder = FrameDecoder::new();
    let opened = async {
        let (mut ws, welcome) = open_api(&dial.url, &dial.token, dial.ca_file.as_deref()).await?;
        let name = welcome.workspaces.iter().find(|w| w.id == dial.workspace).map_or(dial.workspace.clone(), |w| w.name.clone());
        let open = ClientFrame::TunnelOpen(TunnelOpen { id: TUNNEL, workspace: dial.workspace.clone() });
        ws.send(Message::text(serde_json::to_string(&open).expect("frames serialize"))).await?;
        tokio::time::timeout(WAIT, async {
            loop {
                match next_server_frame(&mut ws).await? {
                    ServerFrame::TunnelOpened(TunnelId { id: TUNNEL }) => return Ok(()),
                    ServerFrame::TunnelClosed(c) if c.id == TUNNEL => return Err(refused(&name, c.error)),
                    ServerFrame::Goodbye(e) => bail!("the nexus refused: {}", e.message),
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{name} did not answer"))??;
        // Hello, as a client of that keeper, and its welcome.
        let mut hello = Vec::new();
        Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: dial.kind }).encode_into(&mut hello);
        ws.send(Message::binary(tunnel_data(TUNNEL, &hello))).await?;
        let mut early = Vec::new();
        tokio::time::timeout(WAIT, async {
            loop {
                match ws.next().await {
                    None | Some(Ok(Message::Close(_))) => bail!("{name} closed the connection"),
                    Some(Err(e)) => bail!("the connection to the nexus failed: {e}"),
                    Some(Ok(Message::Binary(data))) => {
                        let Some((TUNNEL, bytes)) = split_tunnel_data(&data) else { continue };
                        decoder.push(bytes);
                        loop {
                            match decoder.next_frame() {
                                Ok(Some(Frame::Keeper(KeeperMsg::Welcome { version, .. }))) if version != PROTOCOL_VERSION => {
                                    bail!("{name} runs another version of Shed (keeper protocol {version}, this one {PROTOCOL_VERSION}); update one of them")
                                }
                                Ok(Some(Frame::Keeper(KeeperMsg::Error(e)))) if e.starts_with("protocol mismatch") => {
                                    bail!("{name} runs another version of Shed ({e}); update one of them")
                                }
                                Ok(Some(frame @ Frame::Keeper(KeeperMsg::Welcome { .. }))) => {
                                    early.push(frame);
                                    return Ok(());
                                }
                                Ok(Some(frame)) => early.push(frame),
                                Ok(None) => break,
                                Err(_) => bail!("{name} runs another version of Shed; update one of them"),
                            }
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(ServerFrame::TunnelClosed(c)) = serde_json::from_str::<ServerFrame>(&text)
                            && c.id == TUNNEL
                        {
                            return Err(refused(&name, c.error));
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("{name}'s keeper did not answer"))??;
        anyhow::Ok((ws, name, early))
    };
    let (ws, name, early) = match opened.await {
        Ok(o) => o,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    for frame in early {
        let _ = frames.send(frame).await;
    }
    let _ = ready.send(Ok(()));
    let why = pipe(ws, &name, &mut decoder, &frames, &mut out, &close).await;
    if let Some(why) = why {
        tracing::warn!("tunnel to {name}: {why}");
        let _ = frames.send(Frame::Keeper(KeeperMsg::Error(format!("{LOST}{why}")))).await;
    }
}

/// How an `Error` frame saying the tunnel ended starts.
pub const LOST: &str = "lost the connection to the workspace: ";

/// Carries bytes until the tunnel ends; why, unless it was asked to close.
async fn pipe(
    ws: Socket,
    name: &str,
    decoder: &mut FrameDecoder,
    frames: &async_channel::Sender<Frame>,
    out: &mut mpsc::Receiver<Vec<u8>>,
    close: &Notify,
) -> Option<String> {
    let (mut sink, mut stream) = ws.split();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                None => return Some("the nexus closed the connection".into()),
                Some(Err(e)) => return Some(e.to_string()),
                Some(Ok(Message::Close(_))) => return Some("the nexus closed the connection".into()),
                Some(Ok(Message::Binary(data))) => {
                    let Some((TUNNEL, bytes)) = split_tunnel_data(&data) else { continue };
                    decoder.push(bytes);
                    loop {
                        match decoder.next_frame() {
                            Ok(Some(frame)) => {
                                if frames.send(frame).await.is_err() {
                                    return None;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                let _ = frames.send(Frame::Keeper(KeeperMsg::Error(format!("{OTHER_VERSION} ({e})")))).await;
                                return Some(format!("a frame from {name} did not read: {e}"));
                            }
                        }
                    }
                }
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ServerFrame>(&text) {
                    Ok(ServerFrame::TunnelClosed(c)) if c.id == TUNNEL => {
                        return Some(c.error.map_or_else(|| format!("{name} closed the connection"), |e| refused(name, Some(e)).to_string()));
                    }
                    Ok(ServerFrame::Goodbye(e)) => return Some(format!("the nexus signed this desktop out: {}", e.message)),
                    _ => {}
                },
                Some(Ok(_)) => {}
            },
            bytes = out.recv() => {
                // The app let go of the connection: close the tunnel.
                let Some(bytes) = bytes else {
                    let _ = sink.send(Message::text(serde_json::to_string(&ClientFrame::TunnelClose(TunnelId { id: TUNNEL })).expect("frames serialize"))).await;
                    let _ = sink.send(Message::Close(None)).await;
                    return None;
                };
                for piece in bytes.chunks(CHUNK) {
                    if let Err(e) = sink.send(Message::binary(tunnel_data(TUNNEL, piece))).await {
                        return Some(e.to_string());
                    }
                }
            }
            _ = close.notified() => {
                let _ = sink.send(Message::text(serde_json::to_string(&ClientFrame::TunnelClose(TunnelId { id: TUNNEL })).expect("frames serialize"))).await;
                let _ = sink.send(Message::Close(None)).await;
                return None;
            }
            _ = ping.tick() => {
                if let Err(e) = sink.send(Message::Ping(Vec::new().into())).await {
                    return Some(e.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_sign_in_is_kept_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed/nexus-client.json");
        let s = SignIn { url: "https://n.example.com".into(), username: "me".into(), device: "d1".into(), token: "t0k".into() };
        s.save_to(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(SignIn::load_from(&path), Some(s));
    }
}
