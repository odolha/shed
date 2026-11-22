//! An in-process keeper running real bash, a link to it, and a fake nexus
//! on a real socket that plays the nexus's side of `/link`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use shed_api::calls::{Call, Reply};
use shed_api::state::WorkspaceState;
use shed_api::topics::{Event, Topic};
use shed_api::wire::*;
use shed_api::{API_VERSION, ApiError};
use shed_keeper::{Keeper, KeeperConfig};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

pub struct Env {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub logbook: PathBuf,
    pub home: PathBuf,
}

/// bash for the tests: `SHED_TEST_BASH`, or the first `bash` on PATH.
pub fn bash() -> PathBuf {
    if let Some(path) = std::env::var_os("SHED_TEST_BASH") {
        return path.into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|d| d.join("bash")).find(|p| p.is_file()).unwrap_or_else(|| "/bin/bash".into())
}

/// A keeper with real bash in `dir`, on a runtime of its own.
pub fn keeper(dir: &Path) -> Env {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let socket = dir.join("shedd.sock");
    let logbook = dir.join("state/logbook.sqlite");
    let cfg = KeeperConfig {
        socket: socket.clone(),
        config_file: dir.join("shed.toml"),
        shell: Some(bash()),
        home: home.clone(),
        exit_when_idle: false,
        logbook: Some(logbook.clone()),
        state_dir: dir.join("state"),
        shed_bin: None,
        env: vec![("HOME".into(), home.clone().into()), ("HISTCONTROL".into(), "ignoreboth".into()), ("PS1".into(), "$ ".into())],
        store: Some(shed_save::Store::new(dir.join("benches"))),
        desktop: None,
        aliases_file: dir.join("aliases.toml"),
        link_secret: None,
        link_log: dir.join("link.log"),
    };
    let listen = socket.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&listen).unwrap();
            Keeper::new(cfg).serve(listener).await.unwrap();
        });
    });
    for _ in 0..250 {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Env { dir: dir.to_path_buf(), socket, logbook, home }
}

/// Starts a link to the keeper of `env`, dialling `url`.
pub fn link(env: &Env, url: String, token: &str, policy: Policy) -> tokio::task::JoinHandle<anyhow::Result<shed_link::Exit>> {
    link_with(env, url, token, policy, false)
}

/// A link that may let Shed's desktop app in (`desktop_clients`).
pub fn link_with(env: &Env, url: String, token: &str, policy: Policy, desktop_clients: bool) -> tokio::task::JoinHandle<anyhow::Result<shed_link::Exit>> {
    tokio::spawn(shed_link::run(options(env, url, token, policy, desktop_clients)))
}

/// A link's options, to change before running it.
pub fn options(env: &Env, url: String, token: &str, policy: Policy, desktop_clients: bool) -> shed_link::Options {
    shed_link::Options {
        socket: env.socket.clone(),
        url,
        token: token.into(),
        policy,
        ca_file: None,
        logbook: env.logbook.clone(),
        pictures: env.dir.join("state/ai-pictures"),
        uploads: env.dir.join("state/ai-files"),
        status_file: Some(env.dir.join("link.json")),
        config_file: None,
        desktop_clients,
        settings: None,
    }
}

pub type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// The nexus's end of a link, faked: frames in and out, by hand.
pub struct FakeNexus {
    pub socket: Socket,
    pub state: Option<WorkspaceState>,
    next_id: u32,
    /// Events and frames read while waiting for something else.
    pub events: Vec<EventFrame>,
    pub notes: Vec<Notification>,
    /// Subscriptions that ended.
    pub ends: Vec<Ended>,
}

/// Listens for one link; the address to give it.
pub async fn listen() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    (listener, url)
}

/// Takes the link's connection, checks its hello and welcomes it.
pub async fn accept(listener: &TcpListener) -> (FakeNexus, LinkHello) {
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(15), listener.accept()).await.expect("the link dials").unwrap();
    let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
    let hello = match read(&mut socket).await {
        LinkFrame::Hello(h) => h,
        other => panic!("not a hello: {other:?}"),
    };
    assert_eq!(hello.api, API_VERSION);
    let welcome = LinkWelcome { api: API_VERSION, nexus: "test".into(), workspace: "w1".into(), name: "desk".into() };
    socket.send(Message::text(serde_json::to_string(&NexusFrame::Welcome(welcome)).unwrap())).await.unwrap();
    (FakeNexus { socket, state: None, next_id: 1, events: Vec::new(), notes: Vec::new(), ends: Vec::new() }, hello)
}

async fn read(socket: &mut Socket) -> LinkFrame {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(20), socket.next()).await.expect("a frame within 20 s").expect("open").unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap_or_else(|e| panic!("{e}: {t}"));
        }
    }
}

impl FakeNexus {
    async fn send(&mut self, frame: NexusFrame) {
        self.socket.send(Message::text(serde_json::to_string(&frame).unwrap())).await.unwrap();
    }

    /// Reads frames until `pick` takes one, keeping states and events.
    pub async fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&LinkFrame, &Option<WorkspaceState>) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let frame = tokio::time::timeout_at(deadline, read(&mut self.socket)).await.unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            match &frame {
                LinkFrame::State(s) => self.state = Some(s.clone()),
                LinkFrame::Event(e) => self.events.push(e.clone()),
                LinkFrame::Notify(n) => self.notes.push(n.clone()),
                LinkFrame::Ended(e) => self.ends.push(e.clone()),
                _ => {}
            }
            if let Some(v) = pick(&frame, &self.state) {
                return v;
            }
        }
    }

    /// A state the test likes.
    pub async fn state_where(&mut self, what: &str, ok: impl Fn(&WorkspaceState) -> bool) -> WorkspaceState {
        if let Some(s) = self.state.as_ref().filter(|s| ok(s)) {
            return s.clone();
        }
        self.until(what, |f, _| match f {
            LinkFrame::State(s) if ok(s) => Some(s.clone()),
            _ => None,
        })
        .await
    }

    pub async fn call(&mut self, call: Call) -> Result<Reply, ApiError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(NexusFrame::Request(Request { id, workspace: String::new(), call })).await;
        let r = self
            .until("a response", |f, _| match f {
                LinkFrame::Response(r) if r.id == id => Some(r.clone()),
                _ => None,
            })
            .await;
        match (r.ok, r.error) {
            (Some(ok), _) => Ok(ok),
            (_, Some(e)) => Err(e),
            _ => panic!("an empty response"),
        }
    }

    pub async fn subscribe(&mut self, topic: Topic) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(NexusFrame::Subscribe(Subscribe { id, workspace: String::new(), topic })).await;
        id
    }

    pub async fn unsubscribe(&mut self, id: u32) {
        self.send(NexusFrame::Unsubscribe(Unsubscribe { id })).await;
    }

    /// The first event of `sub` (seen already or to come) that `pick` takes.
    pub async fn event<T>(&mut self, sub: u32, what: &str, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        if let Some(i) = self.events.iter().position(|e| e.sub == sub && pick(&e.event).is_some()) {
            let e = self.events.remove(i);
            return pick(&e.event).unwrap();
        }
        let found = self
            .until(what, |f, _| match f {
                LinkFrame::Event(e) if e.sub == sub => pick(&e.event),
                LinkFrame::Ended(e) if e.sub == sub => panic!("{what}: the subscription ended: {:?}", e.error),
                _ => None,
            })
            .await;
        self.events.clear();
        found
    }

    /// The subscription's end, with its error.
    pub async fn ended(&mut self, sub: u32) -> Option<ApiError> {
        if let Some(e) = self.ends.iter().find(|e| e.sub == sub) {
            return e.error.clone();
        }
        self.until("the end of a subscription", |f, _| match f {
            LinkFrame::Ended(e) if e.sub == sub => Some(e.error.clone()),
            _ => None,
        })
        .await
    }
}

/// The text of a screen event's rows, joined.
pub fn screen_text(s: &shed_api::topics::Screen) -> String {
    s.lines.iter().map(|r| r.line.spans.iter().map(|sp| sp.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

/// Runs git in `dir`.
pub fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(ok.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ok.stderr));
}

/// A Git repository with one commit (`a.txt`) and a change to it.
pub fn repo(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-q", "-m", "first"]);
    std::fs::write(root.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    root
}
