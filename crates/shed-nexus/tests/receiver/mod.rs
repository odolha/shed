//! A fake push service and ntfy on loopback: answers 201 to anything and
//! keeps what came, for the notification tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::HeaderMap;
use shed_api::http::PushSubscribe;

/// A request that reached the fake server.
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// A fake push service and ntfy: answers 201 to anything, keeps what came.
pub struct Receiver {
    pub base: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Receiver {
    pub async fn start() -> Receiver {
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
        let kept = hits.clone();
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri, headers: HeaderMap, body: axum::body::Bytes| {
            let kept = kept.clone();
            async move {
                kept.lock().unwrap().push(Hit { path: uri.path().to_string(), headers, body: body.to_vec() });
                axum::http::StatusCode::CREATED
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Receiver { base, hits }
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    pub fn on(&self, path: &str) -> Vec<Hit> {
        self.hits().into_iter().filter(|h| h.path == path).collect()
    }

    /// Waits until `n` requests came (at most a few seconds), then a little
    /// longer for any that should not.
    pub async fn wait_for(&self, n: usize) -> Vec<Hit> {
        for _ in 0..100 {
            if self.hits().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.hits()
    }
}

/// A subscription to `path` on the receiver, with a real key to encrypt for.
pub fn subscription(receiver: &Receiver, path: &str) -> PushSubscribe {
    use base64::Engine;
    use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
    let secret = web_push_native::p256::SecretKey::random(&mut web_push_native::p256::elliptic_curve::rand_core::OsRng);
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    PushSubscribe { endpoint: format!("{}{path}", receiver.base), p256dh: key, auth: "AAAAAAAAAAAAAAAAAAAAAA".into() }
}
