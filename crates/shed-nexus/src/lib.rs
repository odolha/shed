//! The nexus (`shed-nexus`): a server that links Shed workspaces to the web
//! and Android apps. It keeps the account and the paired devices, remembers
//! each workspace's last state, relays requests and subscriptions between
//! connectors (`/api`) and workspaces' links (`/link`), sends notifications
//! (live to apps in view, else pushed) and serves the web app. See `docs/architecture.md` ("The
//! nexus") and `docs/nexus-plan.md`.

pub mod auth;
pub mod config;
pub mod geo;
mod http;
pub mod ntfy;
mod pair;
pub mod push;
pub mod raw;
mod relay;
pub mod store;
pub mod web;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::store::Store;

/// The nexus's version, as it tells peers.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything the routes share.
pub struct App {
    pub config: Config,
    pub store: Store,
    hub: relay::Hub,
    guard: auth::Guard,
    filter: geo::Filter,
    push: Option<push::Push>,
    /// The ntfy topic phones read, if any.
    ntfy: Option<ntfy::Ntfy>,
    setup: Mutex<Setup>,
    origins: Vec<String>,
    /// Proxies whose `X-Forwarded-For` is believed.
    proxies: Vec<ipnet::IpNet>,
    /// Turns at argon2 (each run takes ~19 MiB and a core).
    argon: Arc<tokio::sync::Semaphore>,
    /// The web app's files.
    web: web::WebSource,
}

/// Argon2 runs at once, at most.
const ARGON_AT_ONCE: usize = 2;

/// Where the setup link is written while there is no account.
pub const SETUP_FILE: &str = "setup-link.txt";

/// Setting up the account: the code from the log, and the first step's
/// answers until the authenticator is confirmed.
#[derive(Default)]
struct Setup {
    code: Option<String>,
    pending: Option<PendingSetup>,
}

struct PendingSetup {
    username: String,
    password: String,
    totp_secret: String,
}

/// A running nexus.
pub struct Server {
    /// Where it listens (with the real port when the config asked for 0).
    pub addr: SocketAddr,
    /// The one-time setup code, while there is no account.
    pub setup_code: Option<String>,
    app: Arc<App>,
    tasks: Vec<JoinHandle<()>>,
}

impl Server {
    /// Stops serving and writes what is not saved yet.
    pub async fn stop(self) {
        for task in &self.tasks {
            task.abort();
        }
        relay::flush_states(&self.app);
        self.app.hub.close_all();
    }
}

struct ServeLimits {
    connections: usize,
    header_timeout: Duration,
}

/// Serves HTTP/1 and HTTP/2 (with WebSocket upgrades) as `axum::serve`
/// does, with limits it lacks: at most `connections` at once (more wait
/// in the kernel's queue), and a time limit on reading a request's headers,
/// so slow or idle clients cannot hold connections open.
async fn serve(listener: tokio::net::TcpListener, router: axum::Router, limits: ServeLimits) {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use tower_service::Service;
    let slots = Arc::new(tokio::sync::Semaphore::new(limits.connections));
    let mut make = router.into_make_service_with_connect_info::<SocketAddr>();
    loop {
        let Ok(slot) = slots.clone().acquire_owned().await else { return };
        let (tcp, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                // Out of file descriptors and the like: wait, then go on.
                tracing::warn!("accepting a connection: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let _ = tcp.set_nodelay(true);
        let Ok(service) = make.call(addr).await;
        let header_timeout = limits.header_timeout;
        let tcp = FirstBytes { tcp, deadline: Box::pin(tokio::time::sleep(header_timeout)), seen: 0 };
        tokio::spawn(async move {
            let _slot = slot;
            let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            builder.http1().timer(TokioTimer::new()).header_read_timeout(header_timeout);
            builder.http2().timer(TokioTimer::new()).enable_connect_protocol();
            let service = hyper_util::service::TowerToHyperService::new(service);
            if let Err(e) = builder.serve_connection_with_upgrades(TokioIo::new(tcp), service).await {
                tracing::trace!("a connection ended: {e}");
            }
        });
    }
}

/// A connection whose first bytes must come in time. Before the HTTP/1
/// header timeout applies, hyper reads the first bytes to tell HTTP/1 from
/// HTTP/2, with no time limit; a client that sends nothing would hold its
/// connection open forever.
struct FirstBytes {
    tcp: tokio::net::TcpStream,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
    seen: usize,
}

/// The HTTP/2 preface's length: what hyper reads to choose the protocol.
const FIRST_BYTES: usize = 24;

impl tokio::io::AsyncRead for FirstBytes {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::future::Future;
        let this = self.get_mut();
        if this.seen < FIRST_BYTES && this.deadline.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no request in time")));
        }
        let before = buf.filled().len();
        let polled = std::pin::Pin::new(&mut this.tcp).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(())) = &polled {
            this.seen += buf.filled().len() - before;
        }
        polled
    }
}

impl tokio::io::AsyncWrite for FirstBytes {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().tcp).poll_write(cx, buf)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().tcp).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().tcp).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().tcp).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.tcp.is_write_vectored()
    }
}

/// Unix seconds.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Picks the TLS library's crypto (for push requests). Safe to call again.
pub fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Writes a secret file readable by its owner only.
pub fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("writing {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Opens the database, makes the push key if needed, and starts serving.
pub async fn start(config: Config) -> anyhow::Result<Server> {
    config.check()?;
    install_crypto();
    let data = config.data_dir();
    std::fs::create_dir_all(&data).with_context(|| format!("making {}", data.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700));
    }
    let store = Store::open(&data.join("nexus.db"))?;
    #[cfg(unix)]
    for name in ["nexus.db", "nexus.db-wal", "nexus.db-shm"] {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(data.join(name), std::fs::Permissions::from_mode(0o600));
    }
    let push = if config.push.enabled {
        Some(push::Push::load_or_create(
            &data,
            config.push_subject(),
            config.push.allow_http_endpoints,
            config.push.allow_private_endpoints,
        )?)
    } else {
        None
    };
    let ntfy = match config.ntfy()? {
        Some(topic) if config.push.enabled => {
            tracing::info!("notifications for phones go to the ntfy topic {}", topic.url);
            Some(ntfy::Ntfy::new(topic)?)
        }
        _ => None,
    };
    let filter = geo::Filter::new(&config)?;
    let web = web::WebSource::new(config.web_dir.as_deref());
    tracing::info!("{}", web.describe());

    // The setup link goes to a file only its owner reads, not to the log
    // (journals are read by more people, and kept); the code is in the
    // URL's fragment, which browsers do not send, so no proxy logs it.
    let mut setup = Setup::default();
    let setup_file = data.join(SETUP_FILE);
    if store.account()?.is_none() {
        let code = auth::random_chars(20);
        let link = format!("{}/setup#code={code}", config.public_base());
        write_secret(&setup_file, format!("{link}\n").as_bytes())?;
        tracing::warn!("no account yet: the setup link is in {} (readable by this user only)", setup_file.display());
        setup.code = Some(code);
    } else {
        let _ = std::fs::remove_file(&setup_file);
    }
    let setup_code = setup.code.clone();

    let hub = relay::Hub::new(Duration::from_secs(config.request_timeout_secs.max(1)), &store)?;
    let guard = auth::Guard::new(config.login.clone());
    // Where the owner's devices were last seen: not held back when many
    // sign-ins fail.
    for device in store.devices(now())? {
        if let Ok(ip) = device.address.parse() {
            guard.remember(ip);
        }
    }
    let app = Arc::new(App {
        guard,
        origins: config.origins(),
        proxies: config.proxies(),
        argon: Arc::new(tokio::sync::Semaphore::new(ARGON_AT_ONCE)),
        filter,
        push,
        ntfy,
        hub,
        store,
        setup: Mutex::new(setup),
        web,
        config,
    });

    let listener = tokio::net::TcpListener::bind(app.config.listen)
        .await
        .with_context(|| format!("listening on {}", app.config.listen))?;
    let addr = listener.local_addr()?;
    tracing::info!("listening on {addr}; public URL {}", app.config.public_base());
    let router = http::router(app.clone());
    let limits = ServeLimits {
        connections: app.config.max_connections.max(1),
        header_timeout: Duration::from_secs(app.config.header_timeout_secs.max(1)),
    };
    let serve = tokio::spawn(serve(listener, router, limits));
    let mut tasks = vec![serve];
    tasks.extend(relay::background(&app));
    Ok(Server { addr, setup_code, app, tasks })
}
