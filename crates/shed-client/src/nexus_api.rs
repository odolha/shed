//! Calls and subscriptions on another workspace through the nexus, as the
//! web and Android apps make them (`shed_api::calls`, `shed_api::topics`):
//! what the desktop app shows of a remote workspace that is not the keeper's
//! (its projects' files and Git, the logbook, Claude Code's sessions, the
//! review, folders to choose from) comes this way, since this machine's
//! disk is not that workspace's.
//!
//! One `/api` connection, signed in as this desktop, on a thread of its own
//! with a small tokio runtime. It is made when first needed, and made again
//! after a drop while subscriptions are open (each is subscribed again, its
//! first event being the whole thing again); a call made while it is down
//! fails at once. Answers come as futures that any executor can wait on,
//! or blocking (`call_blocking`, never inside a tokio runtime).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use shed_api::calls::{Call, Reply};
use shed_api::topics::{Event, Topic};
use shed_api::wire::{ClientFrame, Request, ServerFrame, Subscribe, Unsubscribe};
use shed_api::{ApiError, ErrorCode};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::nexus_client::{self, PING_EVERY, SignIn, Socket};

/// Reconnecting waits this long at first, doubling up to `MAX_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// What a subscription brings.
#[derive(Clone, Debug, PartialEq)]
pub enum SubEvent {
    Event(Event),
    /// It ended without being asked to (the tab closed, the workspace went
    /// offline, the desktop was signed out); subscribe again to resume.
    Ended(Option<ApiError>),
}

/// Answers calls in tests, at once, without a nexus.
pub type FakeCalls = Arc<dyn Fn(&str, Call) -> Result<Reply, ApiError> + Send + Sync>;

/// A connection to a nexus's `/api` as this desktop. Cheap to clone; the
/// connection ends when the last clone (and subscription) goes.
#[derive(Clone)]
pub struct NexusApi {
    inner: Arc<Inner>,
}

struct Inner {
    cmds: mpsc::UnboundedSender<Cmd>,
    next_sub: AtomicU32,
    fake: Option<Fake>,
}

struct Fake {
    calls: FakeCalls,
    subs: Mutex<Vec<(u32, Topic, async_channel::Sender<SubEvent>)>>,
}

enum Cmd {
    Request { workspace: String, call: Call, reply: oneshot::Sender<Result<Reply, ApiError>> },
    Subscribe { id: u32, workspace: String, topic: Topic, events: async_channel::Sender<SubEvent> },
    Unsubscribe(u32),
}

struct Dial {
    url: String,
    token: String,
    ca_file: Option<PathBuf>,
}

impl NexusApi {
    /// The nexus `sign_in` is for, trusting `ca_file` too (`[nexus]
    /// ca_file`). Nothing connects until the first call or subscription.
    pub fn new(sign_in: &SignIn, ca_file: Option<PathBuf>) -> NexusApi {
        let (cmds, rx) = mpsc::unbounded_channel();
        let dial = Dial { url: sign_in.url.clone(), token: sign_in.token.clone(), ca_file };
        let spawned = std::thread::Builder::new().name("nexus-api".into()).spawn(move || match nexus_client::runtime() {
            Ok(rt) => rt.block_on(run(dial, rx)),
            Err(e) => tracing::warn!("nexus calls: {e:#}"),
        });
        if let Err(e) = spawned {
            tracing::warn!("nexus calls: {e}");
        }
        NexusApi { inner: Arc::new(Inner { cmds, next_sub: AtomicU32::new(1), fake: None }) }
    }

    /// A stand-in for tests: `calls` answers every call at once; events
    /// for subscriptions are sent by the test (`fake_subscriptions`).
    pub fn fake(calls: FakeCalls) -> NexusApi {
        let (cmds, _) = mpsc::unbounded_channel();
        NexusApi { inner: Arc::new(Inner { cmds, next_sub: AtomicU32::new(1), fake: Some(Fake { calls, subs: Mutex::new(Vec::new()) }) }) }
    }

    /// A fake's open subscriptions: their topics, and where their events go.
    pub fn fake_subscriptions(&self) -> Vec<(Topic, async_channel::Sender<SubEvent>)> {
        let Some(fake) = &self.inner.fake else { return Vec::new() };
        let mut subs = fake.subs.lock().unwrap();
        subs.retain(|(_, _, tx)| !tx.is_closed());
        subs.iter().map(|(_, topic, tx)| (topic.clone(), tx.clone())).collect()
    }

    /// One of the nexus's workspaces, by its id.
    pub fn workspace(&self, id: &str) -> RemoteWorkspace {
        RemoteWorkspace { api: self.clone(), id: id.to_string() }
    }
}

/// Another workspace, reached through the nexus: its calls and topics.
#[derive(Clone)]
pub struct RemoteWorkspace {
    api: NexusApi,
    /// The nexus's id for it.
    pub id: String,
}

impl std::fmt::Debug for RemoteWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteWorkspace").field("id", &self.id).finish()
    }
}

impl PartialEq for RemoteWorkspace {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.api.inner, &other.api.inner)
    }
}

/// Why a call has no answer: the connection went.
fn lost() -> ApiError {
    ApiError::new(ErrorCode::Offline, "lost the connection to the nexus")
}

/// A reply of another kind than the call gives.
pub fn unexpected(reply: &Reply) -> ApiError {
    let kind = format!("{reply:?}");
    let kind = kind.split(['(', ' ', '{']).next().unwrap_or_default().to_string();
    ApiError::new(ErrorCode::Failed, format!("the workspace answered {kind}, not what was asked for"))
}

impl RemoteWorkspace {
    /// Calls the workspace; its answer, or why not.
    pub fn call(&self, call: Call) -> impl Future<Output = Result<Reply, ApiError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        match &self.api.inner.fake {
            Some(fake) => {
                let _ = tx.send((fake.calls)(&self.id, call));
            }
            None => {
                let _ = self.api.inner.cmds.send(Cmd::Request { workspace: self.id.clone(), call, reply: tx });
            }
        }
        async move { rx.await.unwrap_or_else(|_| Err(lost())) }
    }

    /// `call`, waiting for it here (not inside a tokio runtime).
    pub fn call_blocking(&self, call: Call) -> Result<Reply, ApiError> {
        let (tx, rx) = oneshot::channel();
        match &self.api.inner.fake {
            Some(fake) => return (fake.calls)(&self.id, call),
            None => {
                let _ = self.api.inner.cmds.send(Cmd::Request { workspace: self.id.clone(), call, reply: tx });
            }
        }
        rx.blocking_recv().unwrap_or_else(|_| Err(lost()))
    }

    /// Subscribes to a topic: its events until the subscription is dropped
    /// (which unsubscribes) or ends.
    pub fn subscribe(&self, topic: Topic) -> Subscription {
        let id = self.api.inner.next_sub.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = async_channel::unbounded();
        match &self.api.inner.fake {
            Some(fake) => fake.subs.lock().unwrap().push((id, topic, tx)),
            None => {
                let _ = self.api.inner.cmds.send(Cmd::Subscribe { id, workspace: self.id.clone(), topic, events: tx });
            }
        }
        Subscription { id, events: rx, api: self.api.clone() }
    }
}

/// An open subscription; dropping it unsubscribes.
pub struct Subscription {
    id: u32,
    pub events: async_channel::Receiver<SubEvent>,
    api: NexusApi,
}

impl Subscription {
    /// The next event; `None` once it has ended (after its `Ended`).
    pub async fn next(&self) -> Option<SubEvent> {
        self.events.recv().await.ok()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        match &self.api.inner.fake {
            Some(fake) => fake.subs.lock().unwrap().retain(|(id, _, _)| *id != self.id),
            None => {
                let _ = self.api.inner.cmds.send(Cmd::Unsubscribe(self.id));
            }
        }
    }
}

/// A subscription as the connection keeps it, to subscribe again after a drop.
struct Sub {
    workspace: String,
    topic: Topic,
    events: async_channel::Sender<SubEvent>,
}

/// How a connection ended.
enum End {
    /// Every handle is gone.
    Done,
    /// Dropped: dial again while subscriptions are open.
    Lost,
    /// The nexus signed this desktop out: no use dialling again.
    Goodbye(ApiError),
}

/// Takes a command while there is no connection: a call fails with `error`,
/// a subscription waits for the next one.
fn while_down(cmd: Cmd, error: &ApiError, subs: &mut HashMap<u32, Sub>) {
    match cmd {
        Cmd::Request { reply, .. } => {
            let _ = reply.send(Err(error.clone()));
        }
        Cmd::Subscribe { id, workspace, topic, events } => {
            subs.insert(id, Sub { workspace, topic, events });
        }
        Cmd::Unsubscribe(id) => {
            subs.remove(&id);
        }
    }
}

async fn run(dial: Dial, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
    let mut subs: HashMap<u32, Sub> = HashMap::new();
    let mut first: Option<Cmd> = None;
    let mut backoff = FIRST_BACKOFF;
    let mut signed_out: Option<ApiError> = None;
    loop {
        // Nothing to do: wait for something.
        if subs.is_empty() && first.is_none() {
            match cmds.recv().await {
                Some(cmd) => first = Some(cmd),
                None => return,
            }
        }
        if let Some(error) = &signed_out {
            for (_, sub) in subs.drain() {
                let _ = sub.events.try_send(SubEvent::Ended(Some(error.clone())));
            }
            if let Some(cmd) = first.take() {
                while_down(cmd, error, &mut subs);
            }
            continue;
        }
        let ws = match nexus_client::open_api(&dial.url, &dial.token, dial.ca_file.as_deref()).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                let error = ApiError::new(ErrorCode::Offline, format!("{e:#}"));
                if let Some(cmd) = first.take() {
                    while_down(cmd, &error, &mut subs);
                }
                while let Ok(cmd) = cmds.try_recv() {
                    while_down(cmd, &error, &mut subs);
                }
                if subs.is_empty() {
                    continue;
                }
                // Subscriptions wait for the next try; calls meanwhile fail.
                let wait = tokio::time::sleep(backoff);
                tokio::pin!(wait);
                loop {
                    tokio::select! {
                        _ = &mut wait => break,
                        cmd = cmds.recv() => match cmd {
                            Some(cmd) => while_down(cmd, &error, &mut subs),
                            None => return,
                        },
                    }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = FIRST_BACKOFF;
        match serve(ws, &mut cmds, &mut subs, first.take()).await {
            End::Done => return,
            End::Lost => {}
            End::Goodbye(e) => {
                tracing::warn!("the nexus signed this desktop out: {}", e.message);
                signed_out = Some(e);
            }
        }
    }
}

fn text(frame: &ClientFrame) -> Message {
    Message::text(serde_json::to_string(frame).expect("frames serialize"))
}

/// Carries calls and subscriptions over one connection until it ends.
async fn serve(ws: Socket, cmds: &mut mpsc::UnboundedReceiver<Cmd>, subs: &mut HashMap<u32, Sub>, first: Option<Cmd>) -> End {
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<u32, oneshot::Sender<Result<Reply, ApiError>>> = HashMap::new();
    let mut next_request = 1u32;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    // Subscriptions from before a drop, again.
    for (id, sub) in subs.iter() {
        let frame = ClientFrame::Subscribe(Subscribe { id: *id, workspace: sub.workspace.clone(), topic: sub.topic.clone() });
        if sink.send(text(&frame)).await.is_err() {
            return End::Lost;
        }
    }
    let mut next_cmd = first;
    loop {
        if let Some(cmd) = next_cmd.take() {
            let frame = match cmd {
                Cmd::Request { workspace, call, reply } => {
                    let id = next_request;
                    next_request = next_request.wrapping_add(1).max(1);
                    pending.insert(id, reply);
                    ClientFrame::Request(Request { id, workspace, call })
                }
                Cmd::Subscribe { id, workspace, topic, events } => {
                    subs.insert(id, Sub { workspace: workspace.clone(), topic: topic.clone(), events });
                    ClientFrame::Subscribe(Subscribe { id, workspace, topic })
                }
                Cmd::Unsubscribe(id) => {
                    if subs.remove(&id).is_none() {
                        continue;
                    }
                    ClientFrame::Unsubscribe(Unsubscribe { id })
                }
            };
            if sink.send(text(&frame)).await.is_err() {
                for (_, reply) in pending.drain() {
                    let _ = reply.send(Err(lost()));
                }
                return End::Lost;
            }
            continue;
        }
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(cmd) => next_cmd = Some(cmd),
                None => {
                    let _ = sink.send(Message::Close(None)).await;
                    return End::Done;
                }
            },
            msg = stream.next() => {
                let frame = match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {
                        for (_, reply) in pending.drain() {
                            let _ = reply.send(Err(lost()));
                        }
                        return End::Lost;
                    }
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<ServerFrame>(&t) {
                        Ok(frame) => frame,
                        Err(_) => continue,
                    },
                    Some(Ok(_)) => continue,
                };
                match frame {
                    ServerFrame::Response(r) => {
                        if let Some(reply) = pending.remove(&r.id) {
                            let answer = match (r.ok, r.error) {
                                (Some(ok), _) => Ok(ok),
                                (None, Some(e)) => Err(e),
                                (None, None) => Err(ApiError::new(ErrorCode::Failed, "an empty answer")),
                            };
                            let _ = reply.send(answer);
                        }
                    }
                    ServerFrame::Event(e) => {
                        let gone = subs.get(&e.sub).is_some_and(|s| s.events.try_send(SubEvent::Event(e.event)).is_err());
                        // Nobody listens any more.
                        if gone {
                            subs.remove(&e.sub);
                            if sink.send(text(&ClientFrame::Unsubscribe(Unsubscribe { id: e.sub }))).await.is_err() {
                                return End::Lost;
                            }
                        }
                    }
                    ServerFrame::Ended(e) => {
                        if let Some(sub) = subs.remove(&e.sub) {
                            let _ = sub.events.try_send(SubEvent::Ended(e.error));
                        }
                    }
                    ServerFrame::Goodbye(e) => {
                        for (_, reply) in pending.drain() {
                            let _ = reply.send(Err(e.clone()));
                        }
                        return End::Goodbye(e);
                    }
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    for (_, reply) in pending.drain() {
                        let _ = reply.send(Err(lost()));
                    }
                    return End::Lost;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shed_api::calls::TabOnly;

    #[test]
    fn a_fake_answers_at_once_and_keeps_subscriptions() {
        let api = NexusApi::fake(Arc::new(|ws, call| match call {
            Call::GitStatus(t) => Ok(Reply::Text(format!("{ws}:{}", t.tab))),
            _ => Err(ApiError::new(ErrorCode::Unsupported, "no")),
        }));
        let w = api.workspace("w1");
        assert_eq!(w.call_blocking(Call::GitStatus(TabOnly { tab: "t".into() })), Ok(Reply::Text("w1:t".into())));
        let sub = w.subscribe(Topic::Project(shed_api::topics::ProjectTopic { tab: "t".into() }));
        let subs = api.fake_subscriptions();
        assert_eq!(subs.len(), 1);
        subs[0].1.send_blocking(SubEvent::Event(Event::GitChanged)).unwrap();
        assert_eq!(sub.events.try_recv(), Ok(SubEvent::Event(Event::GitChanged)));
        drop(sub);
        assert!(api.fake_subscriptions().is_empty());
    }

    #[test]
    fn a_nexus_that_cannot_be_reached_fails_calls_in_words() {
        let sign_in = SignIn { url: "https://127.0.0.1:9".into(), username: "me".into(), device: "d".into(), token: "t".into() };
        let w = NexusApi::new(&sign_in, None).workspace("w1");
        let e = w.call_blocking(Call::GitStatus(TabOnly { tab: "t".into() })).unwrap_err();
        assert_eq!(e.code, ErrorCode::Offline);
        assert!(e.message.contains("cannot reach the nexus"), "{}", e.message);
    }
}
