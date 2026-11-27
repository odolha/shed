//! Relaying between connectors (`/api`) and workspaces' links (`/link`).
//!
//! The hub knows every live connection. A connector's `Request` or
//! `Subscribe` goes to the named workspace's link under an id the nexus
//! picks (unique per link); the link's `Response`s and `Event`s come back
//! under that id and go to the connector under its own. Every subscription is
//! forwarded as is, even when another connector watches the same thing: the
//! link may stream it twice, which keeps the nexus simple and correct (sharing
//! streams can come later).
//!
//! The nexus does not read calls, so it does not know which change something:
//! a workspace's policy (view only) is enforced by its link, which answers
//! `Forbidden`.
//!
//! When a link goes, its subscriptions end (`Ended` with `Offline`) and its
//! requests are answered `Offline`; when a connector goes, its subscriptions
//! are unsubscribed at the links. A request not answered within the timeout
//! is answered `Timeout`. A workspace's `State` is kept as its last snapshot
//! (written to the database every few seconds) and sent to every connector.
//!
//! Tunnels (desktop connectors only) are routed the same way: the connector's
//! tunnel id maps to one the nexus picks for the link, and binary messages
//! (`[u32 id][bytes]`) are passed on with the id swapped. A tunnel whose far
//! side cannot keep up is closed rather than buffered: each connection may
//! have at most `TUNNEL_BACKLOG` bytes of tunnel data waiting. The link is
//! told which device opened a tunnel; every tunnel's end, and every refusal,
//! goes into the audit log (`TunnelAudit`, written by a task of its own).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde_json::value::RawValue;
use shed_api::http::DeviceKind;
use shed_api::wire::{
    ClientHello, Ended, LinkHello, LinkWelcome, Notification, NotificationFrame, NotificationKind, Policy, TunnelClosed, TunnelId,
    LinkTunnelOpen, TunnelDevice, TunnelOpen, Unsubscribe, WorkspaceInfo, split_tunnel_data, tunnel_data,
};
use shed_api::{API_VERSION, ApiError, ErrorCode, MIN_API_VERSION};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::http::{Client, Failure, bearer, cookie};
use crate::raw::*;
use crate::store::{AuditInfo, DeviceRow, Store};
use crate::{App, auth, now};

/// Frames up to this size: pictures for AI prompts travel inside them.
const MAX_MESSAGE: usize = 24 * 1024 * 1024;
/// Frames waiting for a slow connection before it is dropped.
const QUEUE: usize = 4096;
/// How long the first frame (`Hello`) may take.
const HELLO_WAIT: Duration = Duration::from_secs(10);
/// Pings keep proxies from closing quiet sockets; a socket silent for
/// `IDLE` is taken for dead.
const PING: Duration = Duration::from_secs(30);
const IDLE: Duration = Duration::from_secs(90);
/// Notifications a workspace may send: this many a minute, this many at once.
const NOTIFY_PER_MINUTE: u32 = 30;
const NOTIFY_BURST: u32 = 10;
/// Tunnel data waiting for one connection, at most; past it the tunnel
/// that would add more is closed.
const TUNNEL_BACKLOG: usize = 8 * 1024 * 1024;
/// Tunnels a connector may have open at once.
const MAX_TUNNELS: usize = 8;

// --------------------------------------------------------------- the hub

pub struct Hub {
    inner: Mutex<Inner>,
    timeout: Duration,
    /// Tunnels' ends, for `background` to write to the audit log.
    audits: Mutex<Option<mpsc::UnboundedReceiver<TunnelAudit>>>,
}

/// A tunnel that ended or was refused, for the audit log.
#[derive(Debug)]
pub struct TunnelAudit {
    /// `tunnel-close` or `tunnel-refused`.
    pub what: &'static str,
    /// The desktop's device id.
    pub device: String,
    pub workspace: String,
    pub why: String,
}

#[derive(Default)]
struct Inner {
    next_conn: u64,
    /// By workspace id.
    links: HashMap<String, Link>,
    connectors: HashMap<u64, Connector>,
    /// Each workspace's last state.
    states: HashMap<String, Raw>,
    /// States not written to the database yet.
    dirty: HashSet<String>,
    /// The connection each workspace last left with, to tell whether it came
    /// back since.
    left: HashMap<String, u64>,
    audit: Option<mpsc::UnboundedSender<TunnelAudit>>,
}

struct Link {
    conn: u64,
    out: Outbox,
    since: u64,
    policy: Policy,
    version: String,
    next_id: u32,
    /// Nexus id → who asked.
    requests: HashMap<u32, Pending>,
    /// Nexus id → (connector, its subscription id).
    subs: HashMap<u32, (u64, u32)>,
    /// Nexus id → (connector, its tunnel id).
    tunnels: HashMap<u32, (u64, u32)>,
}

struct Pending {
    conn: u64,
    id: u32,
    deadline: Instant,
}

struct Connector {
    device: String,
    out: Outbox,
    /// In view, as it last said (`Visible`).
    visible: bool,
    /// Its subscription id → (workspace, nexus id).
    subs: HashMap<u32, (String, u32)>,
    /// Its tunnel id → (workspace, nexus id).
    tunnels: HashMap<u32, (String, u32)>,
}

/// The way to a connection's socket: frames to send, and a way to close it.
#[derive(Clone)]
pub struct Outbox {
    device: String,
    tx: mpsc::Sender<Message>,
    closer: Arc<Closer>,
    /// Bytes of tunnel data queued and not yet written.
    backlog: Arc<AtomicUsize>,
}

#[derive(Default)]
pub struct Closer {
    notify: Notify,
    /// The `Goodbye` frame to send first, if any.
    goodbye: Mutex<Option<String>>,
}

impl Closer {
    fn close(&self, goodbye: Option<String>) {
        let mut slot = self.goodbye.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = goodbye;
        }
        drop(slot);
        self.notify.notify_one();
    }
}

impl Outbox {
    fn new(device: &str) -> (Outbox, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel(QUEUE);
        (Outbox { device: device.to_string(), tx, closer: Arc::default(), backlog: Arc::default() }, rx)
    }

    /// Queues a frame; a connection that cannot keep up is closed.
    fn send(&self, text: impl Into<Utf8Bytes>) {
        if self.tx.try_send(Message::Text(text.into())).is_err() {
            self.closer.close(None);
        }
    }

    /// Queues a tunnel's data; false (nothing queued) when the connection
    /// is too far behind, so the tunnel is closed instead of buffered.
    fn send_data(&self, id: u32, bytes: &[u8]) -> bool {
        let len = bytes.len() + 4;
        if self.backlog.load(Ordering::Relaxed) + len > TUNNEL_BACKLOG || self.tx.capacity() < QUEUE / 8 {
            return false;
        }
        self.backlog.fetch_add(len, Ordering::Relaxed);
        if self.tx.try_send(Message::Binary(Bytes::from(tunnel_data(id, bytes)))).is_err() {
            self.backlog.fetch_sub(len, Ordering::Relaxed);
            return false;
        }
        true
    }
}

fn tunnel_closed(id: u32, error: Option<ApiError>) -> String {
    RawServerFrame::TunnelClosed(TunnelClosed { id, error }).to_text()
}

fn too_slow() -> ApiError {
    ApiError::new(ErrorCode::Failed, "the tunnel could not keep up and was closed")
}

fn goodbye(error: ApiError) -> String {
    RawServerFrame::Goodbye(error).to_text()
}

impl Hub {
    /// A hub with the workspaces' last states from the database.
    pub fn new(timeout: Duration, store: &Store) -> anyhow::Result<Hub> {
        let mut inner = Inner::default();
        for row in store.workspaces()? {
            if let Some(state) = row.state.and_then(|s| RawValue::from_string(s).ok()) {
                inner.states.insert(row.id, state);
            }
        }
        let (audit, audits) = mpsc::unbounded_channel();
        inner.audit = Some(audit);
        Ok(Hub { inner: Mutex::new(inner), timeout, audits: Mutex::new(Some(audits)) })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- links

    /// A workspace's link connected; one already there is replaced.
    fn link_join(&self, workspace: &str, hello: &LinkHello, out: Outbox) -> u64 {
        let mut inner = self.lock();
        inner.next_conn += 1;
        let conn = inner.next_conn;
        if let Some(old) = inner.links.remove(workspace) {
            old.out.closer.close(Some(goodbye(ApiError::new(ErrorCode::Failed, "another link took over"))));
            inner.drop_link(workspace, old);
        }
        let link = Link {
            conn,
            out,
            since: now(),
            policy: hello.policy,
            version: hello.version.clone(),
            next_id: 0,
            requests: HashMap::new(),
            subs: HashMap::new(),
            tunnels: HashMap::new(),
        };
        inner.links.insert(workspace.to_string(), link);
        conn
    }

    /// A link's connection ended; true if it was the workspace's current one.
    fn link_left(&self, workspace: &str, conn: u64) -> bool {
        let mut inner = self.lock();
        if inner.links.get(workspace).is_none_or(|l| l.conn != conn) {
            return false;
        }
        if let Some(link) = inner.links.remove(workspace) {
            inner.drop_link(workspace, link);
        }
        inner.left.insert(workspace.to_string(), conn);
        true
    }

    /// Whether the workspace is still offline since its link `conn` left.
    fn still_gone(&self, workspace: &str, conn: u64) -> bool {
        let inner = self.lock();
        !inner.links.contains_key(workspace) && inner.left.get(workspace) == Some(&conn)
    }

    /// Handles a frame from a link; a notification to send comes back.
    fn link_frame(&self, workspace: &str, conn: u64, frame: RawLinkFrame) -> Option<Notification> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let link = inner.links.get_mut(workspace).filter(|l| l.conn == conn)?;
        match frame {
            RawLinkFrame::Hello(_) => {}
            RawLinkFrame::State(state) => {
                inner.states.insert(workspace.to_string(), state.clone());
                inner.dirty.insert(workspace.to_string());
                let text = RawServerFrame::State(RawStateFrame { workspace: workspace.to_string(), state }).to_text();
                let text = Utf8Bytes::from(text);
                for connector in inner.connectors.values() {
                    connector.out.send(text.clone());
                }
            }
            RawLinkFrame::Response(mut response) => {
                let pending = link.requests.remove(&response.id)?;
                let connector = inner.connectors.get(&pending.conn)?;
                response.id = pending.id;
                connector.out.send(RawServerFrame::Response(response).to_text());
            }
            RawLinkFrame::Event(mut event) => {
                let (conn, sub) = *link.subs.get(&event.sub)?;
                let connector = inner.connectors.get(&conn)?;
                event.sub = sub;
                connector.out.send(RawServerFrame::Event(event).to_text());
            }
            RawLinkFrame::Ended(ended) => {
                let (conn, sub) = link.subs.remove(&ended.sub)?;
                let connector = inner.connectors.get_mut(&conn)?;
                connector.subs.remove(&sub);
                connector.out.send(RawServerFrame::Ended(Ended { sub, error: ended.error }).to_text());
            }
            RawLinkFrame::Notify(notification) => return Some(notification),
            RawLinkFrame::TunnelOpened(TunnelId { id }) => {
                let (conn, cid) = *link.tunnels.get(&id)?;
                let connector = inner.connectors.get(&conn)?;
                connector.out.send(RawServerFrame::TunnelOpened(TunnelId { id: cid }).to_text());
            }
            RawLinkFrame::TunnelClosed(closed) => {
                let (conn, cid) = link.tunnels.remove(&closed.id)?;
                let connector = inner.connectors.get_mut(&conn)?;
                connector.tunnels.remove(&cid);
                let (what, why) = match &closed.error {
                    Some(e) if e.code == ErrorCode::Forbidden => ("tunnel-refused", format!("by the workspace: {}", e.message)),
                    Some(e) => ("tunnel-close", format!("by the workspace: {}", e.message)),
                    None => ("tunnel-close", "by the workspace".to_string()),
                };
                let device = connector.device.clone();
                connector.out.send(tunnel_closed(cid, closed.error));
                audit_tunnel(&inner.audit, what, &device, workspace, why);
            }
        }
        None
    }

    /// A link's tunnel data, on to its connector.
    fn link_data(&self, workspace: &str, conn: u64, message: &[u8]) {
        let Some((nid, bytes)) = split_tunnel_data(message) else { return };
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(link) = inner.links.get_mut(workspace).filter(|l| l.conn == conn) else { return };
        let Some(&(cconn, cid)) = link.tunnels.get(&nid) else { return };
        let Some(connector) = inner.connectors.get_mut(&cconn) else { return };
        if !connector.out.send_data(cid, bytes) {
            link.tunnels.remove(&nid);
            connector.tunnels.remove(&cid);
            link.out.send(RawNexusFrame::TunnelClose(TunnelId { id: nid }).to_text());
            connector.out.send(tunnel_closed(cid, Some(too_slow())));
            audit_tunnel(&inner.audit, "tunnel-close", &connector.device, workspace, "the desktop could not keep up".into());
        }
    }

    // ---- connectors

    fn connector_join(&self, out: Outbox) -> u64 {
        let mut inner = self.lock();
        inner.next_conn += 1;
        let conn = inner.next_conn;
        inner
            .connectors
            .insert(conn, Connector { device: out.device.clone(), out, visible: false, subs: HashMap::new(), tunnels: HashMap::new() });
        conn
    }

    fn connector_left(&self, conn: u64) {
        let mut inner = self.lock();
        let Some(connector) = inner.connectors.remove(&conn) else { return };
        for (workspace, nid) in connector.subs.values() {
            if let Some(link) = inner.links.get_mut(workspace) {
                link.subs.remove(nid);
                link.out.send(RawNexusFrame::Unsubscribe(Unsubscribe { id: *nid }).to_text());
            }
        }
        for (workspace, nid) in connector.tunnels.values() {
            if let Some(link) = inner.links.get_mut(workspace) {
                link.tunnels.remove(nid);
                link.out.send(RawNexusFrame::TunnelClose(TunnelId { id: *nid }).to_text());
            }
            audit_tunnel(&inner.audit, "tunnel-close", &connector.device, workspace, "the desktop's connection ended".into());
        }
        for link in inner.links.values_mut() {
            link.requests.retain(|_, p| p.conn != conn);
        }
    }

    /// Opens a tunnel to a workspace's link for `device` (the caller
    /// checked it may); `Err` says why not.
    fn tunnel_open(&self, conn: u64, open: &TunnelOpen, device: TunnelDevice) -> Result<(), Option<ApiError>> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(connector) = inner.connectors.get_mut(&conn) else { return Ok(()) };
        // The same id again replaces the tunnel.
        if let Some((workspace, nid)) = connector.tunnels.remove(&open.id) {
            if let Some(link) = inner.links.get_mut(&workspace) {
                link.tunnels.remove(&nid);
                link.out.send(RawNexusFrame::TunnelClose(TunnelId { id: nid }).to_text());
            }
            audit_tunnel(&inner.audit, "tunnel-close", &connector.device, &workspace, "replaced by the desktop".into());
        }
        if connector.tunnels.len() >= MAX_TUNNELS {
            return Err(Some(ApiError::new(ErrorCode::RateLimited, format!("at most {MAX_TUNNELS} tunnels at once"))));
        }
        let Some(link) = inner.links.get_mut(&open.workspace) else { return Err(None) };
        let nid = link.next_id();
        link.tunnels.insert(nid, (conn, open.id));
        connector.tunnels.insert(open.id, (open.workspace.clone(), nid));
        link.out.send(RawNexusFrame::TunnelOpen(LinkTunnelOpen { id: nid, device: Some(device) }).to_text());
        Ok(())
    }

    fn tunnel_close(&self, conn: u64, id: u32) {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(connector) = inner.connectors.get_mut(&conn) else { return };
        let Some((workspace, nid)) = connector.tunnels.remove(&id) else { return };
        connector.out.send(tunnel_closed(id, None));
        if let Some(link) = inner.links.get_mut(&workspace) {
            link.tunnels.remove(&nid);
            link.out.send(RawNexusFrame::TunnelClose(TunnelId { id: nid }).to_text());
        }
        audit_tunnel(&inner.audit, "tunnel-close", &connector.device, &workspace, "by the desktop".into());
    }

    /// A connector's tunnel data, on to the link.
    fn connector_data(&self, conn: u64, message: &[u8]) {
        let Some((cid, bytes)) = split_tunnel_data(message) else { return };
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(connector) = inner.connectors.get_mut(&conn) else { return };
        let Some((workspace, nid)) = connector.tunnels.get(&cid).cloned() else { return };
        let Some(link) = inner.links.get_mut(&workspace) else { return };
        if !link.out.send_data(nid, bytes) {
            link.tunnels.remove(&nid);
            connector.tunnels.remove(&cid);
            link.out.send(RawNexusFrame::TunnelClose(TunnelId { id: nid }).to_text());
            connector.out.send(tunnel_closed(cid, Some(too_slow())));
            audit_tunnel(&inner.audit, "tunnel-close", &connector.device, &workspace, "the workspace could not keep up".into());
        }
    }

    /// Sends a frame to one connector.
    fn reply(&self, conn: u64, frame: RawServerFrame) {
        if let Some(connector) = self.lock().connectors.get(&conn) {
            connector.out.send(frame.to_text());
        }
    }

    /// Forwards a request; false if the workspace is not connected.
    fn request(&self, conn: u64, request: RawRequest) -> bool {
        let mut inner = self.lock();
        let Some(link) = inner.links.get_mut(&request.workspace) else { return false };
        let nid = link.next_id();
        let deadline = Instant::now() + self.timeout;
        link.requests.insert(nid, Pending { conn, id: request.id, deadline });
        let forward = RawRequest { id: nid, workspace: String::new(), call: request.call };
        link.out.send(RawNexusFrame::Request(forward).to_text());
        true
    }

    /// Forwards a subscription; false if the workspace is not connected.
    fn subscribe(&self, conn: u64, sub: RawSubscribe) -> bool {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(connector) = inner.connectors.get_mut(&conn) else { return true };
        // The same id again replaces the subscription.
        if let Some((workspace, nid)) = connector.subs.remove(&sub.id)
            && let Some(link) = inner.links.get_mut(&workspace)
        {
            link.subs.remove(&nid);
            link.out.send(RawNexusFrame::Unsubscribe(Unsubscribe { id: nid }).to_text());
        }
        let Some(link) = inner.links.get_mut(&sub.workspace) else { return false };
        let nid = link.next_id();
        link.subs.insert(nid, (conn, sub.id));
        connector.subs.insert(sub.id, (sub.workspace.clone(), nid));
        let forward = RawSubscribe { id: nid, workspace: String::new(), topic: sub.topic };
        link.out.send(RawNexusFrame::Subscribe(forward).to_text());
        true
    }

    fn unsubscribe(&self, conn: u64, id: u32) {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(connector) = inner.connectors.get_mut(&conn) else { return };
        let Some((workspace, nid)) = connector.subs.remove(&id) else { return };
        if let Some(link) = inner.links.get_mut(&workspace) {
            link.subs.remove(&nid);
            link.out.send(RawNexusFrame::Unsubscribe(Unsubscribe { id: nid }).to_text());
        }
    }

    fn set_visible(&self, conn: u64, visible: bool) {
        if let Some(connector) = self.lock().connectors.get_mut(&conn) {
            connector.visible = visible;
        }
    }

    // ---- everyone

    /// Sends a notification live to every connector, if one is in view;
    /// false (nothing sent) when none is.
    pub fn notify_live(&self, frame: NotificationFrame) -> bool {
        let inner = self.lock();
        if !inner.connectors.values().any(|c| c.visible) {
            return false;
        }
        let text = Utf8Bytes::from(RawServerFrame::Notification(frame).to_text());
        for connector in inner.connectors.values() {
            connector.out.send(text.clone());
        }
        true
    }

    /// Sends a notification live to every connector, in view or not, but
    /// those of device `except` (a security alert).
    pub fn notify_all_but(&self, frame: NotificationFrame, except: Option<&str>) {
        let text = Utf8Bytes::from(RawServerFrame::Notification(frame).to_text());
        for connector in self.lock().connectors.values() {
            if Some(connector.device.as_str()) != except {
                connector.out.send(text.clone());
            }
        }
    }

    /// Devices with a connection in view (being used).
    pub fn visible_devices(&self) -> HashSet<String> {
        self.lock().connectors.values().filter(|c| c.visible).map(|c| c.device.clone()).collect()
    }

    /// Sends a frame to every connector.
    fn broadcast(&self, frame: RawServerFrame) {
        let text = Utf8Bytes::from(frame.to_text());
        for connector in self.lock().connectors.values() {
            connector.out.send(text.clone());
        }
    }

    /// Closes every connection of a device (revoked, signed out).
    pub fn close_device(&self, device: &str, error: ApiError) {
        let inner = self.lock();
        let text = goodbye(error);
        let links = inner.links.values().map(|l| &l.out);
        for out in links.chain(inner.connectors.values().map(|c| &c.out)) {
            if out.device == device {
                out.closer.close(Some(text.clone()));
            }
        }
    }

    pub fn close_all(&self) {
        let inner = self.lock();
        let links = inner.links.values().map(|l| &l.out);
        for out in links.chain(inner.connectors.values().map(|c| &c.out)) {
            out.closer.close(None);
        }
    }

    /// Devices with a live connection.
    pub fn connected_devices(&self) -> HashSet<String> {
        let inner = self.lock();
        let links = inner.links.values().map(|l| l.out.device.clone());
        links.chain(inner.connectors.values().map(|c| c.device.clone())).collect()
    }

    /// Forgets a removed workspace's state.
    pub fn forget(&self, workspace: &str) {
        let mut inner = self.lock();
        inner.states.remove(workspace);
        inner.dirty.remove(workspace);
    }

    /// Answers `Timeout` to requests past their deadline.
    fn expire(&self, at: Instant) {
        let mut inner = self.lock();
        let inner = &mut *inner;
        for link in inner.links.values_mut() {
            let late: Vec<u32> = link.requests.iter().filter(|(_, p)| p.deadline <= at).map(|(id, _)| *id).collect();
            for nid in late {
                let Some(pending) = link.requests.remove(&nid) else { continue };
                if let Some(connector) = inner.connectors.get(&pending.conn) {
                    let error = ApiError::new(ErrorCode::Timeout, "the workspace did not answer in time");
                    let response = RawResponse { id: pending.id, ok: None, error: Some(error) };
                    connector.out.send(RawServerFrame::Response(response).to_text());
                }
            }
        }
    }

    fn take_dirty(&self) -> Vec<(String, String)> {
        let mut inner = self.lock();
        let dirty: Vec<String> = inner.dirty.drain().collect();
        dirty.into_iter().filter_map(|w| inner.states.get(&w).map(|s| (w, s.get().to_string()))).collect()
    }
}

impl Inner {
    /// What is left of a link that went: its subscriptions end and its
    /// requests are answered `Offline`.
    fn drop_link(&mut self, workspace: &str, link: Link) {
        let offline = || ApiError::new(ErrorCode::Offline, "the workspace went offline");
        for pending in link.requests.into_values() {
            if let Some(connector) = self.connectors.get(&pending.conn) {
                let response = RawResponse { id: pending.id, ok: None, error: Some(offline()) };
                connector.out.send(RawServerFrame::Response(response).to_text());
            }
        }
        for (conn, sub) in link.subs.into_values() {
            if let Some(connector) = self.connectors.get_mut(&conn) {
                connector.subs.remove(&sub);
                connector.out.send(RawServerFrame::Ended(Ended { sub, error: Some(offline()) }).to_text());
            }
        }
        for (conn, id) in link.tunnels.into_values() {
            if let Some(connector) = self.connectors.get_mut(&conn) {
                connector.tunnels.remove(&id);
                connector.out.send(tunnel_closed(id, Some(offline())));
                audit_tunnel(&self.audit, "tunnel-close", &connector.device, workspace, "the workspace went offline".into());
            }
        }
    }
}

impl Link {
    fn next_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = self.next_id;
            if !self.requests.contains_key(&id) && !self.subs.contains_key(&id) && !self.tunnels.contains_key(&id) {
                return id;
            }
        }
    }
}

// ---------------------------------------------------------- workspaces

/// The workspaces as connectors see them.
pub fn workspaces(app: &App) -> anyhow::Result<Vec<WorkspaceInfo>> {
    let rows = app.store.workspaces()?;
    let inner = app.hub.lock();
    Ok(rows
        .into_iter()
        .map(|row| match inner.links.get(&row.id) {
            Some(link) => WorkspaceInfo {
                id: row.id,
                name: row.name,
                online: true,
                seen: link.since as u32,
                policy: link.policy,
                version: link.version.clone(),
            },
            None => WorkspaceInfo {
                id: row.id,
                name: row.name,
                online: false,
                seen: row.seen as u32,
                policy: row.policy,
                version: row.version,
            },
        })
        .collect())
}

/// Tells every connector the workspaces changed.
pub fn announce(app: &App) {
    match workspaces(app) {
        Ok(list) => app.hub.broadcast(RawServerFrame::Workspaces(list)),
        Err(e) => tracing::error!("listing workspaces: {e:#}"),
    }
}

/// Writes the states that changed.
pub fn flush_states(app: &App) {
    for (workspace, state) in app.hub.take_dirty() {
        if let Err(e) = app.store.save_state(&workspace, &state, now()) {
            tracing::error!("saving a workspace's state: {e:#}");
        }
    }
}

/// The nexus's chores: timeouts, saving states, dropping what expired and
/// closing connections of devices no longer signed in.
pub fn background(app: &Arc<App>) -> Vec<JoinHandle<()>> {
    let timeouts = {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                app.hub.expire(Instant::now());
            }
        })
    };
    let states = {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            loop {
                tick.tick().await;
                flush_states(&app);
            }
        })
    };
    let cleanup = {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                if let Err(e) = app.store.cleanup(now()) {
                    tracing::error!("cleaning up: {e:#}");
                }
                app.guard.prune();
                // A phone in front of someone is being used: its sign-in
                // does not run out unused while it stays connected.
                for device in app.hub.visible_devices() {
                    let _ = app.store.touch_used(&device, now());
                }
                for device in app.hub.connected_devices() {
                    if !app.store.device_valid(&device, now()).unwrap_or(true) {
                        let message = crate::http::ended_message(&app, app.store.device_ended(&device, now()).ok().flatten());
                        app.hub.close_device(&device, ApiError::new(ErrorCode::Unauthorized, message));
                    }
                }
            }
        })
    };
    let audits = app.hub.audits.lock().unwrap_or_else(|e| e.into_inner()).take().map(|mut rx| {
        let app = app.clone();
        tokio::spawn(async move {
            while let Some(audit) = rx.recv().await {
                write_tunnel_audit(&app, audit);
            }
        })
    });
    let mut tasks = vec![timeouts, states, cleanup];
    tasks.extend(audits);
    tasks
}

/// Queues a tunnel's end for the audit log (written by `background`'s task,
/// outside the hub's lock).
fn audit_tunnel(sink: &Option<mpsc::UnboundedSender<TunnelAudit>>, what: &'static str, device: &str, workspace: &str, why: String) {
    if let Some(sink) = sink {
        let _ = sink.send(TunnelAudit { what, device: device.to_string(), workspace: workspace.to_string(), why });
    }
}

/// Writes a tunnel's end or refusal to the audit log: the device (where it
/// was last seen from), the workspace's name and why.
fn write_tunnel_audit(app: &App, audit: TunnelAudit) {
    let device = app.store.device(&audit.device).ok().flatten();
    let name = app.store.workspaces().ok().and_then(|ws| ws.into_iter().find(|w| w.id == audit.workspace)).map(|w| w.name);
    let detail = format!("{}: {}", name.as_deref().unwrap_or(&audit.workspace), audit.why);
    let info = AuditInfo {
        device: Some(&audit.device),
        address: device.as_ref().map_or("", |d| d.address.as_str()),
        country: device.as_ref().and_then(|d| d.country.as_deref()),
        detail: &detail,
    };
    if let Err(e) = app.store.audit(audit.what, info, now()) {
        tracing::error!("{e:#}");
    }
    tracing::info!("tunnel {} ({}): {detail}", audit.what.trim_start_matches("tunnel-"), device.as_ref().map_or(audit.device.as_str(), |d| d.name.as_str()));
}

// ------------------------------------------------------------ sockets

/// `GET /link`: a workspace's link, with its workspace token.
pub async fn link(
    State(app): State<Arc<App>>,
    client: Client,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Failure> {
    let unauthorized = || Failure::new(ErrorCode::Unauthorized, "not a paired workspace");
    let token = bearer(&headers).ok_or_else(unauthorized)?;
    let device = app.store.device_by_token(&auth::hash_token(token), now())?.ok_or_else(unauthorized)?;
    if device.kind != DeviceKind::Workspace {
        return Err(unauthorized());
    }
    app.store.touch_device(&device.id, &client.address(), client.country.as_deref(), now())?;
    Ok(ws
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run_link(app, socket, device))
        .into_response())
}

/// `GET /api`: a connector: a browser with its session cookie (from an
/// allowed origin) or a phone with its device token.
pub async fn api(
    State(app): State<Arc<App>>,
    client: Client,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Failure> {
    let unauthorized = || Failure::new(ErrorCode::Unauthorized, "not signed in");
    if bearer(&headers).is_none() {
        if cookie(&headers, crate::http::COOKIE).is_none() {
            return Err(unauthorized());
        }
        let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()).unwrap_or_default();
        if !app.origins.iter().any(|allowed| allowed == origin.trim_end_matches('/')) {
            return Err(Failure::new(ErrorCode::Forbidden, "not from an allowed origin"));
        }
    }
    let caller = crate::http::authenticate(&app, &headers, &axum::http::Method::GET)?;
    app.store.touch_device(&caller.device, &client.address(), client.country.as_deref(), now())?;
    let device = app.store.device(&caller.device)?.ok_or_else(unauthorized)?;
    Ok(ws
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run_connector(app, socket, device))
        .into_response())
}

/// The first text frame, if it comes in time.
async fn first_text(socket: &mut WebSocket) -> Option<String> {
    let wait = async {
        loop {
            match socket.recv().await? {
                Ok(Message::Text(text)) => return Some(text.to_string()),
                Ok(Message::Ping(_) | Message::Pong(_)) => continue,
                _ => return None,
            }
        }
    };
    tokio::time::timeout(HELLO_WAIT, wait).await.ok().flatten()
}

async fn say_goodbye(mut socket: WebSocket, error: ApiError) {
    let _ = socket.send(Message::Text(goodbye(error).into())).await;
    let _ = socket.send(Message::Close(None)).await;
}

fn version_ok(api: u32) -> bool {
    (MIN_API_VERSION..=API_VERSION).contains(&api)
}

fn unsupported(api: u32) -> ApiError {
    ApiError::new(
        ErrorCode::Unsupported,
        format!("API version {api} is not accepted; this nexus speaks {MIN_API_VERSION} to {API_VERSION}"),
    )
}

/// What came in on a socket.
enum Incoming<'a> {
    Text(&'a str),
    /// A tunnel's data.
    Binary(&'a [u8]),
}

/// Moves frames both ways until the socket ends or is closed.
async fn pump(socket: &mut WebSocket, out: &Outbox, rx: &mut mpsc::Receiver<Message>, mut on_frame: impl FnMut(Incoming<'_>)) {
    let closer = &out.closer;
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                heard = Instant::now();
                match msg {
                    Some(Ok(Message::Text(text))) => on_frame(Incoming::Text(&text)),
                    Some(Ok(Message::Binary(data))) => on_frame(Incoming::Binary(&data)),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                }
            }
            next = rx.recv() => {
                let Some(message) = next else { break };
                let data = match &message {
                    Message::Binary(b) => b.len(),
                    _ => 0,
                };
                let sent = socket.send(message).await;
                out.backlog.fetch_sub(data, Ordering::Relaxed);
                if sent.is_err() {
                    break;
                }
            }
            _ = closer.notify.notified() => {
                let goodbye = closer.goodbye.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(text) = goodbye {
                    let _ = socket.send(Message::Text(text.into())).await;
                }
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            _ = ping.tick() => {
                if heard.elapsed() > IDLE || socket.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn run_link(app: Arc<App>, mut socket: WebSocket, device: DeviceRow) {
    let hello = match first_text(&mut socket).await.map(|t| RawLinkFrame::parse(&t)) {
        Some(Ok(RawLinkFrame::Hello(hello))) => hello,
        Some(_) => return say_goodbye(socket, ApiError::new(ErrorCode::Invalid, "the first frame must be Hello")).await,
        None => return,
    };
    if !version_ok(hello.api) {
        return say_goodbye(socket, unsupported(hello.api)).await;
    }
    let workspace = device.id.clone();
    if let Err(e) = app.store.workspace_hello(&workspace, hello.policy, &hello.version) {
        tracing::error!("{e:#}");
    }
    let (out, mut rx) = Outbox::new(&device.id);
    let conn = app.hub.link_join(&workspace, &hello, out.clone());
    let welcome =
        LinkWelcome { api: API_VERSION, nexus: crate::VERSION.into(), workspace: workspace.clone(), name: device.name.clone() };
    if socket.send(Message::Text(RawNexusFrame::Welcome(welcome).to_text().into())).await.is_ok() {
        tracing::info!("workspace {:?} connected (Shed {})", device.name, hello.version);
        announce(&app);
        pump(&mut socket, &out, &mut rx, |incoming| match incoming {
            Incoming::Binary(data) => app.hub.link_data(&workspace, conn, data),
            Incoming::Text(text) => match RawLinkFrame::parse(text) {
                Ok(frame) => {
                    if let Some(notification) = app.hub.link_frame(&workspace, conn, frame) {
                        if notification.kind == NotificationKind::Security {
                            // Only the nexus itself raises those.
                            tracing::warn!("workspace {:?} sent a security alert; dropped", device.name);
                        } else if app.guard.take_for("notify", &workspace, NOTIFY_PER_MINUTE, NOTIFY_BURST) {
                            crate::push::notify(&app, &workspace, notification);
                        } else {
                            tracing::debug!("workspace {:?} sends too many notifications; one dropped", device.name);
                        }
                    }
                }
                Err(e) => tracing::debug!("a link sent a frame that does not read ({} bytes): {e}", text.len()),
            },
        })
        .await;
    }
    if app.hub.link_left(&workspace, conn) {
        tracing::info!("workspace {:?} went offline", device.name);
        let _ = app.store.touch_device(&workspace, &device.address, device.country.as_deref(), now());
        flush_states(&app);
        announce(&app);
        offline_notify(&app, workspace, device.name, conn);
    }
}

/// Notifies "went offline" if the workspace has not come back after a while.
fn offline_notify(app: &Arc<App>, workspace: String, name: String, conn: u64) {
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(app.config.push.offline_after_secs)).await;
        if app.hub.still_gone(&workspace, conn) && app.store.device_valid(&workspace, now()).unwrap_or(false) {
            let notification = Notification {
                kind: NotificationKind::WorkspaceOffline,
                title: format!("{name} went offline"),
                body: String::new(),
                bench: None,
                tab: None,
                present: false,
            };
            crate::push::notify(&app, &workspace, notification);
        }
    });
}

async fn run_connector(app: Arc<App>, mut socket: WebSocket, device: DeviceRow) {
    let hello: ClientHello = match first_text(&mut socket).await.map(|t| RawClientFrame::parse(&t)) {
        Some(Ok(RawClientFrame::Hello(hello))) => hello,
        Some(_) => return say_goodbye(socket, ApiError::new(ErrorCode::Invalid, "the first frame must be Hello")).await,
        None => return,
    };
    if !version_ok(hello.api) {
        return say_goodbye(socket, unsupported(hello.api)).await;
    }
    let (out, mut rx) = Outbox::new(&device.id);
    let conn = app.hub.connector_join(out.clone());
    let welcome = match welcome(&app) {
        Ok(welcome) => welcome,
        Err(e) => {
            tracing::error!("{e:#}");
            app.hub.connector_left(conn);
            return say_goodbye(socket, ApiError::new(ErrorCode::Failed, "the nexus failed")).await;
        }
    };
    tracing::debug!("{} connected ({})", device.name, hello.client);
    if socket.send(Message::Text(RawServerFrame::Welcome(welcome).to_text().into())).await.is_ok() {
        pump(&mut socket, &out, &mut rx, |incoming| match incoming {
            Incoming::Binary(data) => app.hub.connector_data(conn, data),
            Incoming::Text(text) => match RawClientFrame::parse(text) {
                Ok(frame) => connector_frame(&app, conn, &device, frame),
                Err(e) => tracing::debug!("a connector sent a frame that does not read: {e}"),
            },
        })
        .await;
    }
    app.hub.connector_left(conn);
}

fn welcome(app: &App) -> anyhow::Result<RawWelcome> {
    let workspaces = workspaces(app)?;
    let known: HashSet<&str> = workspaces.iter().map(|w| w.id.as_str()).collect();
    let states = {
        let inner = app.hub.lock();
        inner
            .states
            .iter()
            .filter(|(id, _)| known.contains(id.as_str()))
            .map(|(id, state)| RawStateFrame { workspace: id.clone(), state: state.clone() })
            .collect()
    };
    Ok(RawWelcome { api: API_VERSION, nexus: crate::VERSION.into(), workspaces, states })
}

/// Why a workspace cannot be reached: not connected, or not there at all.
fn unreachable(app: &App, workspace: &str) -> ApiError {
    match app.store.workspace_exists(workspace) {
        Ok(true) => ApiError::new(ErrorCode::Offline, "the workspace is offline"),
        _ => ApiError::new(ErrorCode::NotFound, "no such workspace"),
    }
}

fn connector_frame(app: &App, conn: u64, device: &DeviceRow, frame: RawClientFrame) {
    match frame {
        RawClientFrame::Hello(_) => {}
        RawClientFrame::Request(request) => {
            let (id, workspace) = (request.id, request.workspace.clone());
            if !app.hub.request(conn, request) {
                let response = RawResponse { id, ok: None, error: Some(unreachable(app, &workspace)) };
                app.hub.reply(conn, RawServerFrame::Response(response));
            }
        }
        RawClientFrame::Subscribe(sub) => {
            let (id, workspace) = (sub.id, sub.workspace.clone());
            if !app.hub.subscribe(conn, sub) {
                app.hub.reply(conn, RawServerFrame::Ended(Ended { sub: id, error: Some(unreachable(app, &workspace)) }));
            }
        }
        RawClientFrame::Unsubscribe(Unsubscribe { id }) => app.hub.unsubscribe(conn, id),
        RawClientFrame::Visible(visible) => app.hub.set_visible(conn, visible),
        RawClientFrame::TunnelOpen(open) => {
            // A tunnel speaks the keeper's own protocol: only Shed's desktop
            // app, signed in as such, may open one.
            if device.kind != DeviceKind::Desktop {
                let error = ApiError::new(ErrorCode::Forbidden, "only Shed's desktop app opens tunnels");
                app.hub.reply(conn, RawServerFrame::TunnelClosed(TunnelClosed { id: open.id, error: Some(error) }));
                let why = format!("by the nexus: a {} may not open tunnels", crate::store::kind_str(device.kind));
                return write_tunnel_audit(app, TunnelAudit { what: "tunnel-refused", device: device.id.clone(), workspace: open.workspace.clone(), why });
            }
            match app.hub.tunnel_open(conn, &open, TunnelDevice { id: device.id.clone(), name: device.name.clone() }) {
                Ok(()) => {
                    let name = app.store.workspaces().ok().and_then(|ws| ws.into_iter().find(|w| w.id == open.workspace)).map(|w| w.name);
                    let detail = name.unwrap_or_else(|| open.workspace.clone());
                    let info = AuditInfo { device: Some(&device.id), address: &device.address, country: device.country.as_deref(), detail: &detail };
                    if let Err(e) = app.store.audit("tunnel-open", info, now()) {
                        tracing::error!("{e:#}");
                    }
                    tracing::info!("{} opened a tunnel to {detail:?}", device.name);
                }
                Err(error) => {
                    let error = error.unwrap_or_else(|| unreachable(app, &open.workspace));
                    let why = format!("by the nexus: {}", error.message);
                    app.hub.reply(conn, RawServerFrame::TunnelClosed(TunnelClosed { id: open.id, error: Some(error) }));
                    write_tunnel_audit(app, TunnelAudit { what: "tunnel-refused", device: device.id.clone(), workspace: open.workspace.clone(), why });
                }
            }
        }
        RawClientFrame::TunnelClose(TunnelId { id }) => app.hub.tunnel_close(conn, id),
    }
}
