//! Push notifications: Web Push (RFC 8030), its payload encrypted for the
//! receiving device (RFC 8291, `aes128gcm`) and signed with the nexus's VAPID
//! key (RFC 8292). Browsers subscribe through their push service; the Android
//! app through a UnifiedPush distributor such as ntfy, which takes the same
//! requests. The push service sees only ciphertext.
//!
//! The VAPID key is made on first start and kept in `vapid.key` in the data
//! folder (0600).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use shed_api::http::PushSubscribe;
use shed_api::wire::{Notification, NotificationKind};
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
    client: reqwest::Client,
}

/// What a device gets, as JSON, once decrypted.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub workspace: String,
    pub workspace_name: String,
    pub bench: Option<String>,
    pub tab: Option<String>,
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
    pub fn load_or_create(dir: &Path, subject: String, allow_http: bool) -> anyhow::Result<Push> {
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
        Push::with_key(key, subject, allow_http)
    }

    pub fn with_key(key: ES256KeyPair, subject: String, allow_http: bool) -> anyhow::Result<Push> {
        let public = URL_SAFE_NO_PAD.encode(key.public_key().public_key().to_bytes_uncompressed());
        crate::install_crypto();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Push { key, public, subject, allow_http, client })
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

/// Pushes `notification` from `workspace` to every device that wants its
/// kind, in the background.
pub fn notify(app: &Arc<App>, workspace: &str, notification: Notification) {
    let Some(_) = &app.push else { return };
    let app = app.clone();
    let workspace = workspace.to_string();
    tokio::spawn(async move {
        let Some(push) = &app.push else { return };
        let subs = match app.store.subscriptions(notification.kind) {
            Ok(subs) => subs,
            Err(e) => {
                tracing::warn!("push: {e:#}");
                return;
            }
        };
        if subs.is_empty() {
            return;
        }
        let name = app.store.device(&workspace).ok().flatten().map(|d| d.name).unwrap_or_default();
        let payload = Payload {
            kind: notification.kind,
            title: clip(&notification.title, 200),
            body: clip(&notification.body, 1500),
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
        let push = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), false).unwrap();
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
        let push = Push::with_key(ES256KeyPair::generate(), "mailto:admin@example.com".into(), false).unwrap();
        let sub = PushSubscribe { endpoint: "http://push.example.com/x".into(), p256dh: "AA".into(), auth: "AA".into() };
        assert!(push.check(&sub).is_err());
        let sub = PushSubscribe { endpoint: "https://push.example.com/x".into(), ..sub };
        assert_eq!(push.check(&sub), Err("p256dh is not a P-256 public key"));
    }

    #[test]
    fn the_key_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let a = Push::load_or_create(dir.path(), "mailto:a@example.com".into(), false).unwrap();
        let b = Push::load_or_create(dir.path(), "mailto:a@example.com".into(), false).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("vapid.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
