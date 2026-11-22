//! Notifications, and pushing them: live to the apps while one is in view
//! (`notify`), else pushed. Web Push (RFC 8030), its payload encrypted for
//! the receiving device (RFC 8291, `aes128gcm`) and signed with the nexus's
//! VAPID key (RFC 8292). Browsers subscribe through their push service; the
//! Android app through a UnifiedPush distributor such as ntfy, which takes
//! the same requests (unless the nexus has a ntfy topic for phones, see
//! `ntfy`). The push service sees only ciphertext.
//!
//! The VAPID key is made on first start and kept in `vapid.key` in the data
//! folder (0600).
//!
//! Endpoints come from signed-in devices, but the nexus should not be made to
//! reach its own network: an endpoint must be on a public address (unless
//! `push.allow_private_endpoints`), checked when subscribing, and again when
//! sending by the resolver the push client connects with, so a name that
//! resolves differently later (DNS rebinding) still cannot lead inside.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use shed_api::http::{DeviceKind, PushPayload as Payload, PushSubscribe};
use shed_api::wire::{Notification, NotificationFrame, NotificationKind};
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};
use web_push_native::p256::PublicKey;
use web_push_native::{Auth, WebPushBuilder};

use crate::App;

pub struct Push {
    key: ES256KeyPair,
    /// The public key, uncompressed, base64url: what browsers subscribe with.
    public: String,
    subject: String,
    allow_http: bool,
    allow_private: bool,
    client: reqwest::Client,
}

/// Whether `ip` is on the public internet: not loopback, private (RFC 1918,
/// unique local), link-local, shared (CGNAT), multicast, unspecified,
/// documentation or otherwise reserved; IPv4 inside IPv6 (mapped, NAT64,
/// 6to4) is judged as IPv4.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || a >= 240) // reserved
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| Ipv4Addr::from(((hi as u32) << 16) | lo as u32);
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
        return public_v4(embedded(s[6], s[7])); // NAT64
    }
    if s[0] == 0x2002 {
        return public_v4(embedded(s[1], s[2])); // 6to4
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link-local
        || (s[0] & 0xffc0) == 0xfec0 // site-local (old)
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x2001 && s[1] == 0) // Teredo
        || s[0..6] == [0, 0, 0, 0, 0, 0]) // IPv4-compatible, and the like
}

/// Resolves names for the push client, keeping only public addresses.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found.into_iter().filter(|a| public_ip(a.ip())).collect();
            if public.is_empty() {
                return Err(format!("{host} has no public address").into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// An endpoint's host and port (the scheme's if none).
fn host_port(endpoint: &str) -> Option<(String, u16)> {
    let uri: axum::http::Uri = endpoint.parse().ok()?;
    let host = uri.host()?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("http") { 80 } else { 443 });
    Some((host, port))
}

/// How a delivery went.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The subscription is gone (404/410): forget it.
    Gone,
    Failed,
}

impl Push {
    /// Loads the VAPID key from `dir/vapid.key`, or makes one.
    pub fn load_or_create(dir: &Path, subject: String, allow_http: bool, allow_private: bool) -> anyhow::Result<Push> {
        let path = dir.join("vapid.key");
        let key = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let bytes = URL_SAFE_NO_PAD.decode(text.trim()).context("vapid.key is not base64url")?;
                ES256KeyPair::from_bytes(&bytes).map_err(|e| anyhow::anyhow!("vapid.key: {e}"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = ES256KeyPair::generate();
                crate::write_secret(&path, URL_SAFE_NO_PAD.encode(key.to_bytes()).as_bytes())?;
                tracing::info!("made a new VAPID key for push");
                key
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Push::with_key(key, subject, allow_http, allow_private)
    }

    pub fn with_key(key: ES256KeyPair, subject: String, allow_http: bool, allow_private: bool) -> anyhow::Result<Push> {
        let public = URL_SAFE_NO_PAD.encode(key.public_key().public_key().to_bytes_uncompressed());
        crate::install_crypto();
        // No proxy from the environment: the connection goes where the
        // resolver says, nowhere else.
        let mut builder =
            reqwest::Client::builder().timeout(Duration::from_secs(15)).redirect(reqwest::redirect::Policy::none()).no_proxy();
        if !allow_private {
            builder = builder.dns_resolver(PublicOnly);
        }
        let client = builder.build()?;
        Ok(Push { key, public, subject, allow_http, allow_private, client })
    }

    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// Checks a subscription before it is kept.
    pub fn check(&self, sub: &PushSubscribe) -> Result<(), &'static str> {
        let https = sub.endpoint.starts_with("https://");
        if !(https || (self.allow_http && sub.endpoint.starts_with("http://"))) {
            return Err("the push endpoint must be an https: URL");
        }
        if sub.endpoint.len() > 2048 || sub.endpoint.parse::<axum::http::Uri>().is_err() {
            return Err("the push endpoint is not a URL");
        }
        keys(sub).map(|_| ())
    }

    /// Checks where an endpoint leads: every address its host has must be
    /// public (unless private endpoints are allowed).
    pub async fn check_address(&self, endpoint: &str) -> Result<(), &'static str> {
        if self.allow_private {
            return Ok(());
        }
        let (host, port) = host_port(endpoint).ok_or("the push endpoint is not a URL")?;
        let not_public = "the push endpoint is not on a public address";
        if let Ok(ip) = host.parse::<IpAddr>() {
            return if public_ip(ip) { Ok(()) } else { Err(not_public) };
        }
        let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| "the push endpoint's host is not known")?
            .collect();
        if found.is_empty() || !found.iter().all(|a| public_ip(a.ip())) {
            return Err(not_public);
        }
        Ok(())
    }

    /// The encrypted, signed request that delivers `payload` to `sub`.
    pub fn request(
        &self,
        sub: &PushSubscribe,
        payload: &[u8],
        urgent: bool,
    ) -> anyhow::Result<axum::http::Request<Vec<u8>>> {
        let (public, auth) = keys(sub).map_err(|e| anyhow::anyhow!(e))?;
        let endpoint = sub.endpoint.parse().context("push endpoint")?;
        let mut request = WebPushBuilder::new(endpoint, public, auth)
            .with_valid_duration(Duration::from_secs(12 * 3600))
            .with_vapid(&self.key, &self.subject)
            .build(payload.to_vec())
            .map_err(|e| anyhow::anyhow!("encrypting the push: {e}"))?;
        let urgency = if urgent { "high" } else { "normal" };
        request.headers_mut().insert("Urgency", axum::http::HeaderValue::from_static(urgency));
        Ok(request)
    }

    pub async fn deliver(&self, sub: &PushSubscribe, payload: &[u8], urgent: bool) -> Delivery {
        // Names are checked by the client's resolver as it connects; an
        // address in the URL is not resolved, so it is checked here.
        if !self.allow_private
            && let Some((host, _)) = host_port(&sub.endpoint)
            && let Ok(ip) = host.parse::<IpAddr>()
            && !public_ip(ip)
        {
            tracing::warn!("push: an endpoint is not on a public address; not sending");
            return Delivery::Failed;
        }
        let request = match self.request(sub, payload, urgent).and_then(|r| Ok(reqwest::Request::try_from(r)?)) {
            Ok(request) => request,
            Err(e) => {
                tracing::warn!("push: {e:#}");
                return Delivery::Failed;
            }
        };
        match self.client.execute(request).await {
            Ok(answer) if answer.status().is_success() => Delivery::Sent,
            Ok(answer) if matches!(answer.status().as_u16(), 404 | 410) => Delivery::Gone,
            Ok(answer) => {
                tracing::warn!("push: the push service answered {}", answer.status());
                Delivery::Failed
            }
            Err(e) => {
                tracing::warn!("push: {e}");
                Delivery::Failed
            }
        }
    }
}

/// The receiver's public key and auth secret.
fn keys(sub: &PushSubscribe) -> Result<(PublicKey, Auth), &'static str> {
    let public = URL_SAFE_NO_PAD
        .decode(sub.p256dh.trim_end_matches('='))
        .ok()
        .and_then(|b| PublicKey::from_sec1_bytes(&b).ok())
        .ok_or("p256dh is not a P-256 public key")?;
    let auth = URL_SAFE_NO_PAD.decode(sub.auth.trim_end_matches('=')).map_err(|_| "auth is not base64url")?;
    if auth.len() != 16 {
        return Err("auth must be 16 bytes");
    }
    Ok((public, Auth::clone_from_slice(&auth)))
}

/// Tells the user about `notification` from `workspace`: live to the
/// connectors while one is in view (nothing pushed then); otherwise, in the
/// background, to the ntfy topic (if there is one and a phone wants the
/// kind) and by Web Push to every subscription that wants it: browsers, and
/// phones too when there is no topic (they read the topic instead).
pub fn notify(app: &Arc<App>, workspace: &str, notification: Notification) {
    let notification = Notification {
        title: clip(&notification.title, TITLE_MAX),
        body: clip(&notification.body, BODY_MAX),
        ..notification
    };
    if app.hub.notify_live(NotificationFrame { workspace: workspace.to_string(), notification: notification.clone() }) {
        return;
    }
    if app.push.is_none() && app.ntfy.is_none() {
        return;
    }
    let app = app.clone();
    let workspace = workspace.to_string();
    tokio::spawn(async move {
        let name = app.store.device(&workspace).ok().flatten().map(|d| d.name).unwrap_or_default();
        if let Some(ntfy) = &app.ntfy {
            match app.store.phones_want(notification.kind) {
                Ok(true) => ntfy.send(&crate::ntfy::message(&workspace, &name, &notification)).await,
                Ok(false) => {}
                Err(e) => tracing::warn!("ntfy: {e:#}"),
            }
        }
        let Some(push) = &app.push else { return };
        let subs = match app.store.subscriptions(notification.kind) {
            Ok(subs) => subs,
            Err(e) => {
                tracing::warn!("push: {e:#}");
                return;
            }
        };
        let phones_by_ntfy = app.ntfy.is_some();
        let subs: Vec<_> = subs.into_iter().filter(|s| !(phones_by_ntfy && s.kind == DeviceKind::Phone)).collect();
        if subs.is_empty() {
            return;
        }
        let payload = Payload {
            kind: notification.kind,
            title: notification.title,
            body: notification.body,
            workspace,
            workspace_name: name,
            bench: notification.bench,
            tab: notification.tab,
        };
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let urgent = notification.kind == NotificationKind::AiWaiting;
        for sub in subs {
            let target = PushSubscribe { endpoint: sub.endpoint, p256dh: sub.p256dh, auth: sub.auth };
            if push.deliver(&target, &bytes, urgent).await == Delivery::Gone {
                tracing::info!("push: a device's subscription is gone; forgetting it");
                let _ = app.store.remove_subscription(&sub.device);
            }
        }
    });
}

/// A notification's title and body, at most (in characters).
pub const TITLE_MAX: usize = 120;
pub const BODY_MAX: usize = 500;

/// At most `max` characters.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_push_native::p256::SecretKey;
    use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;

    #[test]
    fn a_push_decrypts_with_the_receivers_key() {
        crate::install_crypto();
        let push = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), false, false).unwrap();
        // The receiver (a browser or the Android app).
        let secret = SecretKey::random(&mut web_push_native::p256::elliptic_curve::rand_core::OsRng);
        let auth = [7u8; 16];
        let sub = PushSubscribe {
            endpoint: "https://push.example.com/send/abc".into(),
            p256dh: URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes()),
            auth: URL_SAFE_NO_PAD.encode(auth),
        };
        assert!(push.check(&sub).is_ok());
        let request = push.request(&sub, b"{\"title\":\"hi\"}", true).unwrap();
        assert_eq!(request.headers()["content-encoding"], "aes128gcm");
        assert_eq!(request.headers()["urgency"], "high");
        let vapid = request.headers()["authorization"].to_str().unwrap();
        assert!(vapid.starts_with("vapid t="), "{vapid}");
        assert!(vapid.contains(&format!("k={}", push.public_key())));

        let body = request.into_body();
        let plain = web_push_native::decrypt(body, &secret, &Auth::clone_from_slice(&auth)).unwrap();
        assert_eq!(plain, b"{\"title\":\"hi\"}");
    }

    #[test]
    fn bad_subscriptions_are_refused() {
        let push = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), false, false).unwrap();
        let sub = PushSubscribe { endpoint: "http://push.example.com/x".into(), p256dh: "AA".into(), auth: "AA".into() };
        assert!(push.check(&sub).is_err());
        let sub = PushSubscribe { endpoint: "https://push.example.com/x".into(), ..sub };
        assert_eq!(push.check(&sub), Err("p256dh is not a P-256 public key"));
    }

    #[test]
    fn only_public_addresses() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0",
            "224.0.0.1", "255.255.255.255", "::1", "::", "fe80::1", "fd00::1", "ff02::1", "::ffff:127.0.0.1",
            "::ffff:10.0.0.1", "64:ff9b::a00:1", "2002:a00:1::1", "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700::1111", "::ffff:1.1.1.1", "64:ff9b::101:101"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert_eq!(host_port("https://[::1]:8443/x"), Some(("::1".into(), 8443)));
        assert_eq!(host_port("http://ntfy.lan/x"), Some(("ntfy.lan".into(), 80)));
    }

    #[tokio::test]
    async fn endpoints_inside_are_refused() {
        let push = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), false, false).unwrap();
        for endpoint in ["https://127.0.0.1/x", "https://[::1]/x", "https://169.254.169.254/latest", "https://localhost:8080/x"] {
            assert!(push.check_address(endpoint).await.is_err(), "{endpoint}");
        }
        assert!(push.check_address("https://1.1.1.1/x").await.is_ok());
        let lenient = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), true, true).unwrap();
        assert!(lenient.check_address("http://127.0.0.1/x").await.is_ok());
        // Sending to an address inside is refused too, before connecting.
        let sub = PushSubscribe { endpoint: "https://127.0.0.1:1/x".into(), p256dh: String::new(), auth: String::new() };
        assert_eq!(push.deliver(&sub, b"{}", false).await, Delivery::Failed);
    }

    #[tokio::test]
    async fn the_resolver_drops_addresses_inside() {
        use reqwest::dns::Resolve;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(PublicOnly.resolve(name).await.is_err());
    }

    #[test]
    fn the_key_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let a = Push::load_or_create(dir.path(), "mailto:a@example.com".into(), false, false).unwrap();
        let b = Push::load_or_create(dir.path(), "mailto:a@example.com".into(), false, false).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("vapid.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
