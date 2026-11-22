//! Pairing a workspace with a nexus (`shed nexus join`, `status`, `leave`),
//! the pairing kept on disk, and what the running link says about itself;
//! and signing this desktop in to a nexus to show other workspaces
//! (`shed nexus signin`, `signout`), which is apart from the pairing.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use shed_api::ApiError;
use shed_api::http::{DeviceHello, DeviceKind, Login, PairApproved, PairPoll, PairStart, PairStarted, PairState, SignedIn};
use shed_client::nexus_client::SignIn;
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

pub use shed_client::link_status::LinkStatus;

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
    // (Not quoted: an answer may hold the workspace's token.)
    serde_json::from_str(&text).with_context(|| format!("the nexus's answer did not read ({} bytes)", text.len()))
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
        Some(s) => {
            println!("paired as: {} ({}) with {}", s.name, s.workspace, s.url);
            if !n.url.trim().is_empty()
                && let Err(why) = crate::nexus_url(&n.url, &s.url)
            {
                println!("refused: {why}");
            }
        }
        None => println!("paired: no"),
    }
    match SignIn::load() {
        Some(s) => println!("this desktop: signed in to {} as {}", s.url, s.username),
        None => println!("this desktop: not signed in (shed nexus signin)"),
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

/// Signs this desktop in to the nexus at `url` (a device of its own, with
/// a token), to show other workspaces through it. The caller keeps it
/// (`SignIn::save`, in `nexus-client.json`).
pub async fn sign_in(url: &str, username: &str, password: &str, totp: &str, ca_file: Option<&Path>) -> anyhow::Result<SignIn> {
    let url = url.trim().trim_end_matches('/').to_string();
    let client = http(ca_file)?;
    let device = DeviceHello { kind: DeviceKind::Desktop, name: format!("Shed on {}", host_name()) };
    let login = Login { username: username.trim().into(), password: password.into(), totp: totp.trim().into(), device };
    let signed: SignedIn = post(&client, &crate::tls::api_url(&url, "api/v1/login")?, &login).await?;
    let token = signed.token.context("the nexus gave no token (is it older than this Shed?)")?;
    Ok(SignIn { url, username: signed.username, device: signed.device, token })
}

/// `sign_in`, blocking, and kept (for the windows, on a thread of their own).
pub fn sign_in_blocking(url: &str, username: &str, password: &str, totp: &str) -> anyhow::Result<SignIn> {
    let ca = shed_client::nexus_client::configured_ca_file();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let signed = rt.block_on(sign_in(url, username, password, totp, ca.as_deref()))?;
    signed.save().context("keeping the sign-in")?;
    Ok(signed)
}

/// The nexus to sign in to when none is given: the one signed in to
/// before, the one this workspace is paired with, or `shed.toml`'s.
pub fn default_nexus() -> Option<String> {
    SignIn::load()
        .map(|s| s.url)
        .or_else(|| Secret::load().map(|s| s.url))
        .or_else(|| Config::load_from(&Config::path()).ok().map(|c| c.nexus.url).filter(|u| !u.trim().is_empty()))
}

/// Reads a line from the terminal; without echo for secrets.
fn ask(prompt: &str, secret: bool) -> anyhow::Result<String> {
    use std::io::{BufRead, IsTerminal};
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let stdin = std::io::stdin();
    let fd = libc::STDIN_FILENO;
    let mut saved: Option<libc::termios> = None;
    if secret && stdin.is_terminal() {
        // SAFETY: termios is plain data, filled by tcgetattr before use.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) == 0 {
                saved = Some(t);
                t.c_lflag &= !libc::ECHO;
                libc::tcsetattr(fd, libc::TCSANOW, &t);
            }
        }
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(t) = saved {
        // SAFETY: restores the settings read above.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) };
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// `shed nexus signin [URL]`: signs this desktop in, asking for the user,
/// password and a code on the terminal.
pub fn signin(url: Option<String>) -> anyhow::Result<()> {
    let url = match url.or_else(default_nexus) {
        Some(u) => u,
        None => bail!("which nexus? shed nexus signin https://nexus.example.com"),
    };
    let url = url.trim().trim_end_matches('/').to_string();
    crate::tls::api_ws_url(&url)?;
    println!("Signing this desktop in to {url}, to show other workspaces.");
    let username = ask("User: ", false)?;
    let password = ask("Password: ", true)?;
    let totp = ask("Code from the authenticator (or a recovery code): ", false)?;
    let ca = shed_client::nexus_client::configured_ca_file();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let signed = rt.block_on(sign_in(&url, &username, &password, &totp, ca.as_deref()))?;
    signed.save().context("keeping the sign-in")?;
    println!(
        "Signed in as {}. Shed's window lists the nexus's workspaces in its switcher (This PC ▾); a workspace shows there once its shed.toml says desktop_clients = true under [nexus].",
        signed.username
    );
    Ok(())
}

/// How signing out went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignedOut {
    /// The nexus revoked this desktop (its token is dead, its connections
    /// closed) and the sign-in is forgotten here.
    Revoked,
    /// The nexus could not be told (why): the sign-in is forgotten here
    /// only, and the device should be revoked in the web app.
    HereOnly(String),
    NotSignedIn,
}

impl SignedOut {
    /// For people.
    pub fn message(&self) -> String {
        match self {
            SignedOut::Revoked => "Signed out: the nexus no longer accepts this desktop.".into(),
            SignedOut::HereOnly(why) => format!(
                "Signed out here, but the nexus could not be told ({why}). Revoke this desktop in the web app (Devices) to end its token there."
            ),
            SignedOut::NotSignedIn => "Not signed in to a nexus.".into(),
        }
    }
}

/// Revokes a sign-in at its nexus (`POST /api/v1/logout` with its token:
/// the device is revoked and its connections closed). A token the nexus
/// no longer knows counts as revoked.
pub async fn revoke(sign_in: &SignIn, ca_file: Option<&Path>) -> anyhow::Result<()> {
    let client = http(ca_file)?;
    let url = crate::tls::api_url(&sign_in.url, "api/v1/logout")?;
    let response = client
        .post(&url)
        .bearer_auth(&sign_in.token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .with_context(|| format!("reaching {}", sign_in.url))?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(());
    }
    let text = response.text().await.unwrap_or_default();
    let why = serde_json::from_str::<ApiError>(&text).map(|e| e.to_string()).unwrap_or(text);
    bail!("the nexus said {status}: {why}")
}

/// Signs this desktop out: revoked at the nexus first, then forgotten
/// here (forgotten here even when the nexus cannot be reached).
pub async fn sign_out_from(path: &Path, ca_file: Option<&Path>) -> anyhow::Result<SignedOut> {
    let Some(sign_in) = SignIn::load_from(path) else { return Ok(SignedOut::NotSignedIn) };
    let told = revoke(&sign_in, ca_file).await;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("removing the sign-in"),
    }
    Ok(match told {
        Ok(()) => SignedOut::Revoked,
        Err(e) => SignedOut::HereOnly(format!("{e:#}")),
    })
}

/// `sign_out_from` for this desktop's sign-in, blocking (for the windows,
/// on a thread of their own).
pub fn sign_out_blocking() -> anyhow::Result<SignedOut> {
    let ca = shed_client::nexus_client::configured_ca_file();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(sign_out_from(&paths::nexus_client_path(), ca.as_deref()))
}

/// `shed nexus signout`: revokes this desktop's sign-in at the nexus and
/// forgets it here.
pub fn signout() -> anyhow::Result<()> {
    println!("{}", sign_out_blocking()?.message());
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
