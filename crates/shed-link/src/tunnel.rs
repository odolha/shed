//! Tunnels: Shed's desktop app on another computer, signed in to the nexus,
//! speaking this workspace's keeper protocol through it. Each tunnel is a
//! connection of its own to the keeper's socket; bytes go both ways as they
//! are (the desktop says `Hello` and is a client like a local window). The
//! link's first frame there (`ClientMsg::Tunnelled`, with the nexus's device)
//! tells the keeper whose it is, so it is never taken for this machine's
//! window process. It sees everything a window does, private tabs too, so
//! it is allowed only with `[nexus] desktop_clients = true` and full control.
//!
//! Bytes from the keeper wait for the nexus in a bounded channel (the
//! tunnel reads no more until there is room); bytes for the keeper that do
//! not fit in the tunnel's own bounded queue close the tunnel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shed_api::wire::{LinkFrame, TunnelClosed, TunnelId, tunnel_data};
use shed_api::{ApiError, ErrorCode};
use shed_proto::{ClientMsg, Frame, TunnelDevice};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::topics::Out;

/// Pieces of data for the keeper a tunnel queues, at most.
const QUEUE: usize = 256;
/// How much is read from the keeper at once.
const READ: usize = 64 * 1024;

struct Tunnel {
    to_keeper: mpsc::Sender<Vec<u8>>,
    task: JoinHandle<()>,
}

/// The open tunnels, by the nexus's ids.
pub(crate) struct Tunnels {
    socket: PathBuf,
    open: HashMap<u32, Tunnel>,
    /// Tunnels that ended by themselves, to forget.
    ended_tx: mpsc::UnboundedSender<u32>,
}

impl Tunnels {
    pub(crate) fn new(socket: &Path) -> (Tunnels, mpsc::UnboundedReceiver<u32>) {
        let (ended_tx, ended_rx) = mpsc::unbounded_channel();
        (Tunnels { socket: socket.to_path_buf(), open: HashMap::new(), ended_tx }, ended_rx)
    }

    /// Opens tunnel `id` to the keeper; it says `TunnelOpened` once
    /// connected, `TunnelClosed` when it ends.
    pub(crate) fn open(&mut self, id: u32, device: Option<TunnelDevice>, out: Out, data: mpsc::Sender<Vec<u8>>) {
        self.close(id);
        let (to_keeper, from_nexus) = mpsc::channel(QUEUE);
        let socket = self.socket.clone();
        let ended = self.ended_tx.clone();
        match &device {
            Some(d) => info!(id, device = %d.id, "{} opened a tunnel to the keeper", d.name),
            None => info!(id, "a desktop opened a tunnel to the keeper"),
        }
        let task = tokio::spawn(async move {
            let error = run(id, device, &socket, &out, data, from_nexus).await.err();
            if let Some(e) = &error {
                debug!(id, "tunnel ended: {}", e.message);
            } else {
                debug!(id, "tunnel ended");
            }
            let _ = out.send(LinkFrame::TunnelClosed(TunnelClosed { id, error }));
            let _ = ended.send(id);
        });
        self.open.insert(id, Tunnel { to_keeper, task });
    }

    /// Bytes from the desktop for the keeper. A tunnel that cannot take
    /// them is closed (and the nexus told).
    pub(crate) fn data(&mut self, id: u32, bytes: Vec<u8>, out: Option<&Out>) {
        let Some(tunnel) = self.open.get(&id) else { return };
        if tunnel.to_keeper.try_send(bytes).is_err() {
            self.close(id);
            if let Some(out) = out {
                let error = ApiError::new(ErrorCode::Failed, "the keeper could not keep up; the tunnel was closed");
                let _ = out.send(LinkFrame::TunnelClosed(TunnelClosed { id, error: Some(error) }));
            }
        }
    }

    /// Ends a tunnel without a word to the nexus (it asked, or is gone).
    pub(crate) fn close(&mut self, id: u32) {
        if let Some(tunnel) = self.open.remove(&id) {
            tunnel.task.abort();
        }
    }

    /// A tunnel ended by itself: forgets it.
    pub(crate) fn ended(&mut self, id: u32) {
        if self.open.get(&id).is_some_and(|t| t.task.is_finished()) {
            self.open.remove(&id);
        }
    }

    pub(crate) fn close_all(&mut self) {
        for (_, tunnel) in self.open.drain() {
            tunnel.task.abort();
        }
    }
}

impl Drop for Tunnels {
    fn drop(&mut self) {
        self.close_all();
    }
}

async fn run(
    id: u32,
    device: Option<TunnelDevice>,
    socket: &Path,
    out: &Out,
    data: mpsc::Sender<Vec<u8>>,
    mut from_nexus: mpsc::Receiver<Vec<u8>>,
) -> Result<(), ApiError> {
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|e| ApiError::new(ErrorCode::Failed, format!("cannot reach the keeper: {e}")))?;
    let (mut rd, mut wr) = stream.into_split();
    // Whose connection this is, before any of the desktop's bytes.
    let said = Frame::Client(ClientMsg::Tunnelled { device }).encode();
    wr.write_all(&said).await.map_err(|e| ApiError::new(ErrorCode::Failed, format!("the keeper's socket: {e}")))?;
    let _ = out.send(LinkFrame::TunnelOpened(TunnelId { id }));
    let mut buf = vec![0u8; READ];
    loop {
        tokio::select! {
            n = rd.read(&mut buf) => match n {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    if data.send(tunnel_data(id, &buf[..n])).await.is_err() {
                        return Ok(());
                    }
                }
                Err(e) => return Err(ApiError::new(ErrorCode::Failed, format!("the keeper's socket: {e}"))),
            },
            bytes = from_nexus.recv() => match bytes {
                Some(bytes) => {
                    if let Err(e) = wr.write_all(&bytes).await {
                        return Err(ApiError::new(ErrorCode::Failed, format!("the keeper's socket: {e}")));
                    }
                }
                None => return Ok(()),
            },
        }
    }
}
