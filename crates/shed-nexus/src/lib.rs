//! The nexus (`shed-nexus`): a server that links Shed workspaces to the web
//! and Android apps. It keeps the account and the paired devices, remembers
//! each workspace's last state, relays requests and subscriptions between
//! connectors (`/api`) and workspaces' links (`/link`), sends push
//! notifications and serves the web app. See `docs/architecture.md` ("The
//! nexus") and `docs/nexus-plan.md`.

pub mod auth;
pub mod config;
pub mod geo;
mod http;
mod pair;
pub mod push;
pub mod raw;
mod relay;
pub mod store;

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
    setup: Mutex<Setup>,
    origins: Vec<String>,
}

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
        Some(push::Push::load_or_create(&data, config.push_subject(), config.push.allow_http_endpoints)?)
    } else {
        None
    };
    let filter = geo::Filter::new(&config)?;

    let mut setup = Setup::default();
    if store.account()?.is_none() {
        let code = auth::random_chars(20);
        tracing::warn!("no account yet: set it up at {}/setup?code={code}", config.public_base());
        setup.code = Some(code);
    }
    let setup_code = setup.code.clone();

    let hub = relay::Hub::new(Duration::from_secs(config.request_timeout_secs.max(1)), &store)?;
    let app = Arc::new(App {
        guard: auth::Guard::new(config.login.clone()),
        origins: config.origins(),
        filter,
        push,
        hub,
        store,
        setup: Mutex::new(setup),
        config,
    });

    let listener = tokio::net::TcpListener::bind(app.config.listen)
        .await
        .with_context(|| format!("listening on {}", app.config.listen))?;
    let addr = listener.local_addr()?;
    tracing::info!("listening on {addr}; public URL {}", app.config.public_base());
    let router = http::router(app.clone());
    let serve = tokio::spawn(async move {
        let service = router.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, service).await {
            tracing::error!("serving: {e}");
        }
    });
    let mut tasks = vec![serve];
    tasks.extend(relay::background(&app));
    Ok(Server { addr, setup_code, app, tasks })
}
