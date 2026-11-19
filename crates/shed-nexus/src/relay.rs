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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde_json::value::RawValue;
use shed_api::http::DeviceKind;
use shed_api::wire::{
    ClientHello, Ended, LinkHello, LinkWelcome, Notification, NotificationKind, Policy, Unsubscribe, WorkspaceInfo,
};
use shed_api::{API_VERSION, ApiError, ErrorCode, MIN_API_VERSION};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::http::{Client, Failure, bearer, cookie};
use crate::raw::*;
use crate::store::{DeviceRow, Store};
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
/// A workspace must stay offline this long before the push says so.
const OFFLINE_PUSH_AFTER: Duration = Duration::from_secs(120);

// --------------------------------------------------------------- the hub

pub struct Hub {
    inner: Mutex<Inner>,
    timeout: Duration,
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
}

struct Pending {
    conn: u64,
    id: u32,
    deadline: Instant,
}

struct Connector {
    device: String,
    out: Outbox,
    /// Its subscription id → (workspace, nexus id).
    subs: HashMap<u32, (String, u32)>,
}

/// The way to a connection's socket: frames to send, and a way to close it.
#[derive(Clone)]
pub struct Outbox {
    device: String,
    tx: mpsc::Sender<Utf8Bytes>,
    closer: Arc<Closer>,
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
    fn new(device: &str) -> (Outbox, mpsc::Receiver<Utf8Bytes>) {
        let (tx, rx) = mpsc::channel(QUEUE);
        (Outbox { device: device.to_string(), tx, closer: Arc::default() }, rx)
    }

    /// Queues a frame; a connection that cannot keep up is closed.
    fn send(&self, text: impl Into<Utf8Bytes>) {
        if self.tx.try_send(text.into()).is_err() {
            self.closer.close(None);
        }
    }
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
        Ok(Hub { inner: Mutex::new(inner), timeout })
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
            inner.drop_link(old);
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
            inner.drop_link(link);
        }
        inner.left.insert(workspace.to_string(), conn);
        true
    }

    /// Whether the workspace is still offline since its link `conn` left.
    fn still_gone(&self, workspace: &str, conn: u64) -> bool {
        let inner = self.lock();
        !inner.links.contains_key(workspace) && inner.left.get(workspace) == Some(&conn)
    }

    /// Handles a frame from a link; a notification to push comes back.
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
        }
        None
    }

    // ---- connectors

    fn connector_join(&self, out: Outbox) -> u64 {
        let mut inner = self.lock();
        inner.next_conn += 1;
        let conn = inner.next_conn;
        inner.connectors.insert(conn, Connector { device: out.device.clone(), out, subs: HashMap::new() });
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
        for link in inner.links.values_mut() {
            link.requests.retain(|_, p| p.conn != conn);
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

    // ---- everyone

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
    fn drop_link(&mut self, link: Link) {
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
    }
}

impl Link {
    fn next_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = self.next_id;
            if !self.requests.contains_key(&id) && !self.subs.contains_key(&id) {
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
                for device in app.hub.connected_devices() {
                    if !app.store.device_valid(&device, now()).unwrap_or(true) {
                        app.hub.close_device(&device, ApiError::new(ErrorCode::Unauthorized, "signed out"));
                    }
                }
            }
        })
    };
    vec![timeouts, states, cleanup]
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
    let device = app.store.device_by_token(&auth::hash_token(token))?.ok_or_else(unauthorized)?;
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

/// Moves frames both ways until the socket ends or is closed.
async fn pump(
    socket: &mut WebSocket,
    rx: &mut mpsc::Receiver<Utf8Bytes>,
    closer: &Closer,
    mut on_text: impl FnMut(&str),
) {
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                heard = Instant::now();
                match msg {
                    Some(Ok(Message::Text(text))) => on_text(&text),
                    Some(Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                }
            }
            out = rx.recv() => {
                let Some(text) = out else { break };
                if socket.send(Message::Text(text)).await.is_err() {
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
    let closer = out.closer.clone();
    let conn = app.hub.link_join(&workspace, &hello, out);
    let welcome =
        LinkWelcome { api: API_VERSION, nexus: crate::VERSION.into(), workspace: workspace.clone(), name: device.name.clone() };
    if socket.send(Message::Text(RawNexusFrame::Welcome(welcome).to_text().into())).await.is_ok() {
        tracing::info!("workspace {:?} connected (Shed {})", device.name, hello.version);
        announce(&app);
        pump(&mut socket, &mut rx, &closer, |text| match RawLinkFrame::parse(text) {
            Ok(frame) => {
                if let Some(notification) = app.hub.link_frame(&workspace, conn, frame) {
                    crate::push::notify(&app, &workspace, notification);
                }
            }
            Err(e) => tracing::debug!("a link sent a frame that does not read: {e}"),
        })
        .await;
    }
    if app.hub.link_left(&workspace, conn) {
        tracing::info!("workspace {:?} went offline", device.name);
        let _ = app.store.touch_device(&workspace, &device.address, device.country.as_deref(), now());
        flush_states(&app);
        announce(&app);
        offline_push(&app, workspace, device.name, conn);
    }
}

/// Pushes "went offline" if the workspace has not come back after a while.
fn offline_push(app: &Arc<App>, workspace: String, name: String, conn: u64) {
    if app.push.is_none() {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(OFFLINE_PUSH_AFTER).await;
        if app.hub.still_gone(&workspace, conn) && app.store.device_valid(&workspace, now()).unwrap_or(false) {
            let notification = Notification {
                kind: NotificationKind::WorkspaceOffline,
                title: format!("{name} went offline"),
                body: String::new(),
                bench: None,
                tab: None,
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
    let closer = out.closer.clone();
    let conn = app.hub.connector_join(out);
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
        pump(&mut socket, &mut rx, &closer, |text| match RawClientFrame::parse(text) {
            Ok(frame) => connector_frame(&app, conn, frame),
            Err(e) => tracing::debug!("a connector sent a frame that does not read: {e}"),
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

fn connector_frame(app: &App, conn: u64, frame: RawClientFrame) {
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
    }
}
