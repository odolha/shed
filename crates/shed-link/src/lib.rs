//! `shed link`: a workspace's link to a nexus (see `docs/nexus-plan.md`).
//!
//! The link is a client of the local keeper (`ClientKind::Link`), like a
//! window with no UI, and dials out to the nexus over WSS with the
//! workspace's token. It publishes the workspace's state (`state`), serves
//! the calls connectors make (`calls`), streams what they watch (`topics`,
//! `screen`), raises notifications (`notify`) and keeps the workspace's
//! policy: view only or full control, and private tabs never leave.
//!
//! One task (`run`) owns what changes: the tree, subscriptions, the
//! terminals watched. Calls that read files, Git or the logbook run on
//! their own tasks with a copy of what they need (`calls::Ctx`).

mod calls;
mod keeper;
mod nexus;
pub mod pairing;
mod paths;
mod run;
mod screen;
mod state;
mod tls;
mod topics;
mod notify;

use std::path::PathBuf;

pub use run::run;
use shed_api::wire::Policy;

/// What a link needs to run.
#[derive(Clone, Debug)]
pub struct Options {
    /// The keeper's socket.
    pub socket: PathBuf,
    /// The nexus's address as configured (`https://…`; `ws://…` for tests).
    pub url: String,
    /// The workspace's token, from pairing.
    pub token: String,
    pub policy: Policy,
    /// Certificates to trust besides the usual roots (PEM).
    pub ca_file: Option<PathBuf>,
    /// The keeper's logbook, read only.
    pub logbook: PathBuf,
    /// Where the keeper keeps AI prompts' pictures.
    pub pictures: PathBuf,
    /// Where to say how the link is doing (`shed nexus status`).
    pub status_file: Option<PathBuf>,
    /// `shed.toml`: the link stops when its `[nexus]` table changes, so the
    /// keeper starts it again with the new settings.
    pub config_file: Option<PathBuf>,
}

/// Why the link stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The keeper went away.
    KeeperGone,
    /// The nexus refused the link (unpaired, revoked, another API version).
    Refused(String),
    /// The settings changed: to be started again with the new ones.
    Restart,
}

/// `shed link`: reads the settings and the pairing, and runs until the
/// keeper goes away. Returns the process's exit code.
pub fn main() -> i32 {
    let config = match shed_model::config::Config::load_from(&shed_model::config::Config::path()) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("cannot read the settings: {e}");
            return 1;
        }
    };
    if !config.nexus.active() {
        tracing::info!("no nexus set up, or it is turned off; nothing to do");
        return 0;
    }
    let Some(secret) = pairing::Secret::load() else {
        tracing::warn!("not paired with a nexus (shed nexus join URL)");
        return shed_proto::LINK_REFUSED;
    };
    let opts = Options {
        socket: shed_model::paths::socket_path(),
        url: config.nexus.url.clone(),
        token: secret.token,
        policy: policy_of(config.nexus.control),
        ca_file: config.nexus.ca_file.clone(),
        logbook: shed_logbook::default_path(),
        pictures: shed_model::paths::state_dir().join("ai-pictures"),
        status_file: Some(shed_model::paths::link_status_path()),
        config_file: Some(shed_model::config::Config::path()),
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot start: {e}");
            return 1;
        }
    };
    let code = match rt.block_on(run(opts)) {
        Ok(Exit::KeeperGone) => {
            tracing::info!("the keeper stopped; so does the link");
            0
        }
        Ok(Exit::Restart) => {
            tracing::info!("the nexus settings changed; stopping to start again with them");
            75
        }
        Ok(Exit::Refused(why)) => {
            tracing::error!("the nexus refused this workspace: {why}. Pair it again with `shed nexus join URL`.");
            shed_proto::LINK_REFUSED
        }
        Err(e) => {
            tracing::error!("{e:#}");
            1
        }
    };
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    code
}

/// The API's policy for a `[nexus] control` setting.
pub fn policy_of(control: shed_model::config::NexusControl) -> Policy {
    match control {
        shed_model::config::NexusControl::Full => Policy::Full,
        shed_model::config::NexusControl::View => Policy::View,
    }
}
