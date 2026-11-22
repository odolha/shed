//! The nexus's settings: `nexus.toml`. Every field has a default, so an empty
//! file (or none) gives a nexus on `127.0.0.1:7870` that keeps its data under
//! `$XDG_DATA_HOME/shed-nexus`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where to listen. Behind a reverse proxy, keep it on loopback.
    pub listen: SocketAddr,
    /// The address people and workspaces reach the nexus at, as they type it
    /// (`https://nexus.example.com`). Used for the setup link, the default
    /// WebSocket origin and the push contact.
    pub public_url: String,
    /// The database and the push keys. Default: `$XDG_DATA_HOME/shed-nexus`.
    pub data_dir: Option<PathBuf>,
    /// The built web app, served at `/` (any path that is not a file gets
    /// `index.html`). None: no web app, only the API.
    pub web_dir: Option<PathBuf>,
    /// How long a browser stays signed in.
    pub session_days: u32,
    /// Take the client's address from `X-Forwarded-For` when the request
    /// comes from a trusted proxy: one in `trusted_proxies`, or loopback if
    /// that list is empty. Only behind a proxy you run.
    pub trusted_proxy: bool,
    /// The proxies (address ranges, `127.0.0.1/32`, `10.0.0.5/32`) whose
    /// `X-Forwarded-For` is believed. The client is the right-most address
    /// in it that is not one of them. Empty: loopback, when `trusted_proxy`.
    pub trusted_proxies: Vec<String>,
    /// Session cookies only over HTTPS. Turn off for a local, plain-HTTP dev
    /// nexus that is not on `localhost`.
    pub secure_cookies: bool,
    /// Pages that may open the `/api` WebSocket with a session cookie
    /// (`https://nexus.example.com`). Default: the public URL's origin.
    pub allowed_origins: Vec<String>,
    /// Address ranges allowed to reach the nexus at all (`10.0.0.0/8`,
    /// `2001:db8::/32`). Empty: any.
    pub allowed_networks: Vec<String>,
    /// How long a workspace has to answer a request.
    pub request_timeout_secs: u64,
    /// The Content-Security-Policy sent with every answer.
    pub content_security_policy: String,
    /// Connections open at once; more wait until one closes.
    pub max_connections: usize,
    /// How long a client may take to send a request's headers (and how long
    /// an idle connection is kept between requests), in seconds.
    pub header_timeout_secs: u64,
    pub geo: GeoConfig,
    pub login: LoginConfig,
    pub push: PushConfig,
}

/// An optional country filter, from a GeoIP CSV file.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeoConfig {
    /// A CSV of address ranges and countries: `start,end,country` per line,
    /// addresses as text or numbers (db-ip's "IP to Country Lite", IP2Location
    /// LITE DB1). Also used to show where devices sign in from.
    pub database: Option<PathBuf>,
    /// Two-letter country codes allowed in. Empty: no filter.
    pub countries: Vec<String>,
    /// Let in addresses the database does not know (loopback and private
    /// addresses are always let in).
    pub allow_unknown: bool,
}

/// Limits on signing in and other unauthenticated calls. An "address" is
/// an IPv4 address or an IPv6 /64 (what one household or server gets).
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginConfig {
    /// Failed sign-ins (or setup codes) from one address before it is locked
    /// out; also wrong one-time codes per signed-in device (revoking,
    /// pairing, the password), counted apart.
    pub max_failures: u32,
    /// How long failures count, and how long a lockout lasts.
    pub lockout_minutes: u32,
    /// Failed sign-ins from everywhere together, within `lockout_minutes`,
    /// before the nexus slows down: then addresses that never signed in
    /// share `slow_per_minute` attempts (waiting their turn), while those
    /// that did are not held back.
    pub global_max_failures: u32,
    /// Sign-in attempts per minute, for everyone together, from addresses
    /// that never signed in, while the nexus is slowed down.
    pub slow_per_minute: u32,
    /// Pairing requests per address per minute.
    pub pair_per_minute: u32,
}

/// Notifications when no app is in view: Web Push to browsers, and to
/// phones either a ntfy topic (`ntfy_url`) or UnifiedPush (Web Push through
/// a distributor such as ntfy). While an app is in view, notifications go to
/// the apps live instead, whatever these say.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PushConfig {
    /// Off: nothing is pushed (neither Web Push nor ntfy); apps in view
    /// still get notifications live.
    pub enabled: bool,
    /// Who push services can contact about this nexus: `mailto:…` or an
    /// `https:` URL. Default: the public URL.
    pub subject: Option<String>,
    /// Accept `http:` push endpoints (a ntfy on a private network). Off: only
    /// `https:`.
    pub allow_http_endpoints: bool,
    /// Accept push endpoints on private, loopback or link-local addresses (a
    /// ntfy on a private network). Off: only public addresses, checked when
    /// subscribing and again when sending.
    pub allow_private_endpoints: bool,
    /// A ntfy topic to publish to when no app is in view
    /// (`https://ntfy.example.com/alerts-shed`): phones subscribe to it in
    /// the ntfy app, and get no UnifiedPush then. The env var
    /// `SHED_NEXUS_NTFY_URL` wins over it; the publishing account's access
    /// token comes only from `SHED_NEXUS_NTFY_TOKEN` (kept in the service's
    /// env file, not here), sent as `Authorization: Bearer`. Which kinds go
    /// there: those any signed-in phone wants (all, with no phone).
    pub ntfy_url: Option<String>,
    /// How long a workspace must stay offline before that is notified, in
    /// seconds.
    pub offline_after_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: SocketAddr::from(([127, 0, 0, 1], 7870)),
            public_url: "http://127.0.0.1:7870".into(),
            data_dir: None,
            web_dir: None,
            session_days: 7,
            trusted_proxy: false,
            trusted_proxies: Vec::new(),
            secure_cookies: true,
            allowed_origins: Vec::new(),
            allowed_networks: Vec::new(),
            request_timeout_secs: 60,
            content_security_policy: DEFAULT_CSP.into(),
            max_connections: 1024,
            header_timeout_secs: 10,
            geo: GeoConfig::default(),
            login: LoginConfig::default(),
            push: PushConfig::default(),
        }
    }
}

impl Default for LoginConfig {
    fn default() -> Self {
        LoginConfig { max_failures: 5, lockout_minutes: 15, global_max_failures: 50, slow_per_minute: 6, pair_per_minute: 5 }
    }
}

impl Default for PushConfig {
    fn default() -> Self {
        PushConfig {
            enabled: true,
            subject: None,
            allow_http_endpoints: false,
            allow_private_endpoints: false,
            ntfy_url: None,
            offline_after_secs: 120,
        }
    }
}

/// Only the nexus's own files; inline styles for the web app's components;
/// pictures may be data or blob URLs; no frames, no plugins. The web app's
/// `<base href="/">` must count (`base-uri 'self'`), or a page loaded at a
/// deep link (`/w/…/b/…`) looks for its scripts under that path.
pub const DEFAULT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; worker-src 'self'; \
     manifest-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

impl Config {
    /// Reads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let config: Config = toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        config.check()?;
        Ok(config)
    }

    /// Settings that cannot work, refused at start rather than later.
    pub fn check(&self) -> anyhow::Result<()> {
        if !(self.public_url.starts_with("https://") || self.public_url.starts_with("http://")) {
            anyhow::bail!("public_url must start with https:// or http://");
        }
        for net in &self.allowed_networks {
            net.parse::<ipnet::IpNet>().with_context(|| format!("allowed_networks: {net}"))?;
        }
        for net in &self.trusted_proxies {
            parse_net(net).with_context(|| format!("trusted_proxies: {net}"))?;
        }
        if self.max_connections == 0 {
            anyhow::bail!("max_connections must be at least 1");
        }
        if let Some(subject) = &self.push.subject
            && !(subject.starts_with("mailto:") || subject.starts_with("https://"))
        {
            anyhow::bail!("push.subject must be a mailto: or https: URL");
        }
        if let Some(url) = &self.push.ntfy_url {
            check_ntfy_url(url).context("push.ntfy_url")?;
        }
        if self.session_days == 0 {
            anyhow::bail!("session_days must be at least 1");
        }
        Ok(())
    }

    /// The default config file: `$XDG_CONFIG_HOME/shed-nexus/nexus.toml`.
    pub fn default_path() -> PathBuf {
        xdg_dir("XDG_CONFIG_HOME", ".config").join("shed-nexus").join("nexus.toml")
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(|| xdg_dir("XDG_DATA_HOME", ".local/share").join("shed-nexus"))
    }

    /// The public URL without a trailing slash.
    pub fn public_base(&self) -> &str {
        self.public_url.trim_end_matches('/')
    }

    /// Origins allowed on `/api` with a cookie.
    pub fn origins(&self) -> Vec<String> {
        if !self.allowed_origins.is_empty() {
            return self.allowed_origins.iter().map(|o| o.trim_end_matches('/').to_string()).collect();
        }
        vec![origin_of(&self.public_url)]
    }

    pub fn networks(&self) -> Vec<ipnet::IpNet> {
        self.allowed_networks.iter().filter_map(|n| n.parse().ok()).collect()
    }

    /// The proxies whose `X-Forwarded-For` is believed: `trusted_proxies`,
    /// or loopback when only `trusted_proxy` is on; none when neither is.
    pub fn proxies(&self) -> Vec<ipnet::IpNet> {
        if !self.trusted_proxies.is_empty() {
            return self.trusted_proxies.iter().filter_map(|n| parse_net(n).ok()).collect();
        }
        if self.trusted_proxy {
            return ["127.0.0.0/8", "::1/128"].iter().filter_map(|n| n.parse().ok()).collect();
        }
        Vec::new()
    }

    /// The ntfy topic to publish to, and the token to publish with: the URL
    /// from `SHED_NEXUS_NTFY_URL` or `push.ntfy_url`, the token from
    /// `SHED_NEXUS_NTFY_TOKEN`.
    pub fn ntfy(&self) -> anyhow::Result<Option<NtfyTopic>> {
        self.ntfy_with(|k| std::env::var(k).ok())
    }

    /// `ntfy`, with the environment from `env`.
    pub fn ntfy_with(&self, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<NtfyTopic>> {
        let env = |k: &str| env(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let Some(url) = env("SHED_NEXUS_NTFY_URL").or_else(|| self.push.ntfy_url.clone()) else { return Ok(None) };
        let url = url.trim().to_string();
        check_ntfy_url(&url).context("the ntfy topic (SHED_NEXUS_NTFY_URL or push.ntfy_url)")?;
        let token = env("SHED_NEXUS_NTFY_TOKEN");
        if token.is_none() {
            tracing::warn!(url = %url, "ntfy topic without SHED_NEXUS_NTFY_TOKEN (fine only if the topic allows anonymous publishing)");
        }
        Ok(Some(NtfyTopic { url, token }))
    }

    /// Who push services can contact.
    pub fn push_subject(&self) -> String {
        match &self.push.subject {
            Some(subject) => subject.clone(),
            None => self.public_base().to_string(),
        }
    }
}

/// A ntfy topic and the token to publish to it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtfyTopic {
    /// `https://ntfy.example.com/alerts-shed`.
    pub url: String,
    pub token: Option<String>,
}

/// A topic's URL: `http(s)://host[:port]/topic`, no user or password in it
/// (the token goes in a header, and the URL is shown to signed-in apps).
fn check_ntfy_url(url: &str) -> anyhow::Result<()> {
    let uri: axum::http::Uri = url.parse().map_err(|_| anyhow::anyhow!("{url:?} is not a URL"))?;
    if !matches!(uri.scheme_str(), Some("https" | "http")) || uri.host().is_none() {
        anyhow::bail!("{url:?} must be an https:// (or http://) URL");
    }
    if uri.authority().is_some_and(|a| a.as_str().contains('@')) {
        anyhow::bail!("{url:?}: no user or password in the URL (the token comes from SHED_NEXUS_NTFY_TOKEN)");
    }
    if uri.path().trim_matches('/').is_empty() {
        anyhow::bail!("{url:?} must name a topic, like https://ntfy.example.com/alerts-shed");
    }
    Ok(())
}

/// A range (`10.0.0.0/8`), or one address (`10.0.0.5`).
fn parse_net(net: &str) -> anyhow::Result<ipnet::IpNet> {
    let net = net.trim();
    if let Ok(n) = net.parse::<ipnet::IpNet>() {
        return Ok(n);
    }
    let ip: std::net::IpAddr = net.parse().map_err(|_| anyhow::anyhow!("not an address or range"))?;
    Ok(ipnet::IpNet::from(ip))
}

/// `scheme://host[:port]` of a URL.
pub fn origin_of(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let host = rest.split('/').next().unwrap_or_default();
    format!("{scheme}://{host}")
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            home.join(fallback)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.listen.port(), 7870);
        assert_eq!(config.session_days, 7);
        assert!(config.secure_cookies);
        assert_eq!(config.origins(), vec!["http://127.0.0.1:7870".to_string()]);
    }

    #[test]
    fn origins_come_from_the_public_url() {
        let config: Config = toml::from_str("public_url = \"https://nexus.example.com/\"").unwrap();
        assert_eq!(config.origins(), vec!["https://nexus.example.com".to_string()]);
        assert_eq!(config.public_base(), "https://nexus.example.com");
    }

    #[test]
    fn the_sample_reads() {
        let config: Config = toml::from_str(include_str!("../../../packaging/nexus/nexus.toml")).unwrap();
        config.check().unwrap();
        assert!(config.trusted_proxy);
        assert_eq!(config.public_base(), "https://nexus.example.com");
    }

    #[test]
    fn trusted_proxies_default_to_loopback() {
        let none: Config = toml::from_str("").unwrap();
        assert!(none.proxies().is_empty());
        let loopback: Config = toml::from_str("trusted_proxy = true").unwrap();
        let nets = loopback.proxies();
        assert!(nets.iter().any(|n| n.contains(&"127.0.0.1".parse::<std::net::IpAddr>().unwrap())));
        assert!(nets.iter().any(|n| n.contains(&"::1".parse::<std::net::IpAddr>().unwrap())));
        let listed: Config = toml::from_str("trusted_proxies = [\"10.0.0.5\", \"fd00::/8\"]").unwrap();
        listed.check().unwrap();
        assert_eq!(listed.proxies().len(), 2);
        let bad: Config = toml::from_str("trusted_proxies = [\"nope\"]").unwrap();
        assert!(bad.check().is_err());
    }

    #[test]
    fn the_ntfy_topic_comes_from_the_env_first() {
        let config: Config = toml::from_str("[push]\nntfy_url = \"https://ntfy.example.com/alerts-shed\"").unwrap();
        config.check().unwrap();
        let none = |_: &str| None;
        assert_eq!(
            config.ntfy_with(none).unwrap(),
            Some(NtfyTopic { url: "https://ntfy.example.com/alerts-shed".into(), token: None })
        );
        let env = |k: &str| match k {
            "SHED_NEXUS_NTFY_URL" => Some("https://ntfy.other.org/alerts-x ".into()),
            "SHED_NEXUS_NTFY_TOKEN" => Some("tk_secret".into()),
            _ => None,
        };
        assert_eq!(
            config.ntfy_with(env).unwrap(),
            Some(NtfyTopic { url: "https://ntfy.other.org/alerts-x".into(), token: Some("tk_secret".into()) })
        );
        // A token alone is no topic; an empty variable does not count.
        let token_only = |k: &str| (k == "SHED_NEXUS_NTFY_TOKEN").then(|| "tk".to_string());
        assert_eq!(Config::default().ntfy_with(token_only).unwrap(), None);
        let empty = |k: &str| (k == "SHED_NEXUS_NTFY_URL").then(String::new);
        assert_eq!(config.ntfy_with(empty).unwrap().unwrap().url, "https://ntfy.example.com/alerts-shed");
        let bad = |k: &str| (k == "SHED_NEXUS_NTFY_URL").then(|| "https://me:pw@ntfy.example.com/x".to_string());
        assert!(config.ntfy_with(bad).is_err());
        for url in ["ntfy.example.com/x", "https://ntfy.example.com/", "ftp://ntfy.example.com/x"] {
            let config: Config = toml::from_str(&format!("[push]\nntfy_url = \"{url}\"")).unwrap();
            assert!(config.check().is_err(), "{url}");
        }
    }

    #[test]
    fn bad_settings_are_refused() {
        let config: Config = toml::from_str("allowed_networks = [\"nope\"]").unwrap();
        assert!(config.check().is_err());
        assert!(toml::from_str::<Config>("lisen = \"x\"").is_err());
    }
}
