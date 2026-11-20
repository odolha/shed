//! Pairing a workspace with a nexus (`shed nexus join`, `status`, `leave`),
//! the pairing kept on disk, and what the running link says about itself.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use shed_api::ApiError;
use shed_api::http::{PairApproved, PairPoll, PairStart, PairStarted, PairState};
use shed_model::config::Config;
use shed_model::paths;

/// The pairing: kept in `nexus.json` in Shed's data folder, readable only
/// by you, and not with the `.toml` files backups copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    /// The nexus it was made with.
    pub url: String,
    /// The workspace's id and name there.
    pub workspace: String,
    pub name: String,
    pub token: String,
}

impl Secret {
    pub fn load() -> Option<Secret> {
        Self::load_from(&paths::nexus_secret_path())
    }

    pub fn load_from(path: &Path) -> Option<Secret> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
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
}

/// How the link is doing, as it last wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkStatus {
    pub pid: u32,
    /// `connecting`, `connected`, `offline` or `refused`.
    pub state: String,
    /// The workspace's name at the nexus, once welcomed.
    #[serde(default)]
    pub name: Option<String>,
    /// Why it is not connected, if it says.
    #[serde(default)]
    pub error: Option<String>,
    /// Since when, Unix seconds.
    pub since: u64,
}

impl LinkStatus {
    pub fn write(&self, path: &Path) {
        let _ = paths::write_whole(path, serde_json::to_string(self).expect("serializes"));
    }

    /// The running link's status; `None` when no link runs.
    pub fn read(path: &Path) -> Option<LinkStatus> {
        let s: LinkStatus = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        shed_client::keeper_conn::alive(s.pid).then_some(s)
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is writable for its whole length.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return "workspace".into();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// An HTTP client for the nexus, with the link's TLS settings.
fn http(ca_file: Option<&Path>) -> anyhow::Result<reqwest::Client> {
    let tls = crate::tls::client_config(ca_file)?;
    reqwest::Client::builder()
        .use_preconfigured_tls((*tls).clone())
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("shed/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("an HTTP client")
}

async fn post<B: Serialize, R: for<'de> Deserialize<'de>>(client: &reqwest::Client, url: &str, body: &B) -> anyhow::Result<R> {
    let response = client.post(url).json(body).send().await.with_context(|| format!("reaching {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let why = serde_json::from_str::<ApiError>(&text).map(|e| e.to_string()).unwrap_or(text);
        bail!("the nexus said {status}: {why}");
    }
    serde_json::from_str(&text).with_context(|| format!("the nexus's answer did not read: {}", text.chars().take(200).collect::<String>()))
}

/// How a pairing ended.
pub enum Paired {
    Approved(PairApproved),
    Rejected,
    Expired,
}

/// Asks the nexus at `url` to pair this workspace as `name`; `shown` gets
/// the code to approve in the web app. Polls every two seconds until the
/// request is approved, rejected or expired.
pub async fn pair(url: &str, name: &str, ca_file: Option<&Path>, shown: impl FnOnce(&PairStarted)) -> anyhow::Result<Paired> {
    let client = http(ca_file)?;
    let started: PairStarted = post(&client, &crate::tls::api_url(url, "api/v1/pair/start")?, &PairStart { name: name.into(), version: env!("CARGO_PKG_VERSION").into() }).await?;
    shown(&started);
    let poll_url = crate::tls::api_url(url, "api/v1/pair/poll")?;
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let state: PairState = match post(&client, &poll_url, &PairPoll { poll: started.poll.clone() }).await {
            Ok(s) => s,
            // A moment's trouble reaching it is not the end of pairing.
            Err(e) if now() < started.expires as u64 => {
                tracing::debug!("polling: {e:#}");
                continue;
            }
            Err(e) => return Err(e),
        };
        match state {
            PairState::Pending if now() > started.expires as u64 + 5 => return Ok(Paired::Expired),
            PairState::Pending => {}
            PairState::Approved(a) => return Ok(Paired::Approved(a)),
            PairState::Rejected => return Ok(Paired::Rejected),
            PairState::Expired => return Ok(Paired::Expired),
        }
    }
}

/// `shed nexus join URL [--name NAME]`.
pub fn join(url: &str, name: Option<String>) -> anyhow::Result<()> {
    let url = url.trim().trim_end_matches('/').to_string();
    crate::tls::link_url(&url)?;
    let config = Config::load_from(&Config::path()).map_err(anyhow::Error::msg)?;
    let name = name.unwrap_or_else(host_name);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let ca = config.nexus.ca_file.clone();
    let result = rt.block_on(pair(&url, &name, ca.as_deref(), |s| {
        println!("Pairing code: {}", s.code);
        println!("Approve it in the web app at {url} (Devices), checking that it shows the same code.");
        println!("Waiting…");
    }))?;
    match result {
        Paired::Approved(a) => {
            let secret = Secret { url: url.clone(), workspace: a.workspace, name: a.name.clone(), token: a.token };
            secret.save_to(&paths::nexus_secret_path()).context("keeping the pairing")?;
            Config::set_value(&Config::path(), Some("nexus"), "url", toml_edit::Value::from(url.as_str())).map_err(anyhow::Error::msg)?;
            println!("Linked as {:?}. Shed's keeper starts the link within a few seconds (shed nexus status).", a.name);
            Ok(())
        }
        Paired::Rejected => bail!("the pairing was rejected"),
        Paired::Expired => bail!("the pairing code expired; run `shed nexus join` again"),
    }
}

/// `shed nexus status`.
pub fn status() -> anyhow::Result<()> {
    let config = Config::load_from(&Config::path()).map_err(anyhow::Error::msg)?;
    let n = &config.nexus;
    if n.url.trim().is_empty() {
        println!("nexus: not set up (shed nexus join URL)");
    } else {
        let on = if n.active() { "on" } else { "off" };
        let control = match n.control {
            shed_model::config::NexusControl::Full => "full control",
            shed_model::config::NexusControl::View => "view only",
        };
        println!("nexus: {} ({on}, {control})", n.url);
    }
    match Secret::load() {
        Some(s) => println!("paired as: {} ({})", s.name, s.workspace),
        None => println!("paired: no"),
    }
    match LinkStatus::read(&paths::link_status_path()) {
        Some(s) => {
            let detail = s.error.map(|e| format!(": {e}")).unwrap_or_default();
            println!("link: {} (pid {}, for {}s){detail}", s.state, s.pid, now().saturating_sub(s.since));
        }
        None => println!("link: not running"),
    }
    Ok(())
}

/// `shed nexus leave`: forgets the pairing and the nexus (the keeper stops
/// the link). The nexus still lists the workspace until it is removed there.
pub fn leave() -> anyhow::Result<()> {
    let secret = paths::nexus_secret_path();
    let had = secret.exists();
    if had {
        std::fs::remove_file(&secret).with_context(|| format!("removing {}", secret.display()))?;
    }
    let config = Config::path();
    if config.exists() {
        Config::remove_value(&config, "nexus", "url").map_err(anyhow::Error::msg)?;
    }
    if had {
        println!("Unlinked: the pairing is forgotten here. Remove the workspace in the web app (Devices) too.");
    } else {
        println!("Not paired with a nexus.");
    }
    Ok(())
}

/// Where the pairing is kept (for messages).
pub fn secret_path() -> PathBuf {
    paths::nexus_secret_path()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_pairing_is_kept_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shed/nexus.json");
        let s = Secret { url: "https://n.example.com".into(), workspace: "w1".into(), name: "pc".into(), token: "t0k".into() };
        s.save_to(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(Secret::load_from(&path), Some(s.clone()));
        s.save_to(&path).unwrap();
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "no temporary file left");
    }
}
