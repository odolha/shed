//! An in-process nexus on a real socket, with a plain HTTP client, TOTP codes
//! from the account's secret, and WebSocket peers standing in for a link and
//! connectors.

#![allow(dead_code)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use shed_api::http::*;
use shed_api::wire::*;
use shed_api::{API_VERSION, ApiError};
use shed_nexus::config::Config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub const ORIGIN: &str = "http://app.test";
pub const PASSWORD: &str = "a long enough password";

pub struct Nexus {
    pub server: shed_nexus::Server,
    pub base: String,
    pub dir: tempfile::TempDir,
    pub http: reqwest::Client,
}

pub async fn start() -> Nexus {
    start_with(|_| {}).await
}

pub async fn start_with(change: impl FnOnce(&mut Config)) -> Nexus {
    shed_nexus::install_crypto();
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: Some(dir.path().to_path_buf()),
        allowed_origins: vec![ORIGIN.into()],
        ..Config::default()
    };
    change(&mut config);
    let server = shed_nexus::start(config).await.unwrap();
    let base = format!("http://{}", server.addr);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    Nexus { server, base, dir, http }
}

/// How a request signs in.
#[derive(Clone, Debug)]
pub enum Auth {
    None,
    Cookie { token: String, csrf: Option<String> },
    Bearer(String),
}

impl Nexus {
    pub async fn get_raw(&self, path: &str, auth: &Auth) -> reqwest::Response {
        self.send(self.http.get(format!("{}{path}", self.base)), auth).await
    }

    pub async fn post_raw(&self, path: &str, body: &impl Serialize, auth: &Auth) -> reqwest::Response {
        self.send(self.http.post(format!("{}{path}", self.base)).json(body), auth).await
    }

    async fn send(&self, mut req: reqwest::RequestBuilder, auth: &Auth) -> reqwest::Response {
        match auth {
            Auth::None => {}
            Auth::Cookie { token, csrf } => {
                req = req.header("Cookie", format!("shed_session={token}"));
                if let Some(csrf) = csrf {
                    req = req.header("X-Shed-Csrf", csrf);
                }
            }
            Auth::Bearer(token) => req = req.bearer_auth(token),
        }
        req.send().await.unwrap()
    }

    /// A GET that must work.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, auth: &Auth) -> T {
        let answer = self.get_raw(path, auth).await;
        assert_eq!(answer.status(), StatusCode::OK, "GET {path}");
        answer.json().await.unwrap()
    }

    /// A POST that must work.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize, auth: &Auth) -> T {
        let answer = self.post_raw(path, body, auth).await;
        let status = answer.status();
        let text = answer.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "POST {path}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// A POST that must fail with `status`; its error.
    pub async fn post_err(&self, path: &str, body: &impl Serialize, auth: &Auth, status: StatusCode) -> ApiError {
        let answer = self.post_raw(path, body, auth).await;
        assert_eq!(answer.status(), status, "POST {path}");
        answer.json().await.unwrap()
    }

    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.server.addr)
    }
}

/// Codes from the account's authenticator. Each time step is taken once, in
/// order, so it waits for the next step when the ones at hand are used.
pub struct Authenticator {
    pub totp: totp_rs::TOTP,
    last: Option<u64>,
}

impl Authenticator {
    pub fn new(secret: &str) -> Authenticator {
        Authenticator { totp: shed_nexus::auth::totp(secret, "me").unwrap(), last: None }
    }

    pub async fn code(&mut self) -> String {
        loop {
            let now = unix();
            let step = now / 30;
            let next = match self.last {
                Some(last) => (last + 1).max(step - 1),
                None => step - 1,
            };
            if next <= step + 1 {
                self.last = Some(next);
                return self.totp.generate(next * 30);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

pub fn unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub fn browser() -> DeviceHello {
    DeviceHello { kind: DeviceKind::Browser, name: "Firefox on Linux".into() }
}

pub fn phone() -> DeviceHello {
    DeviceHello { kind: DeviceKind::Phone, name: "Pixel".into() }
}

/// The account, set up, with a browser signed in.
pub struct Account {
    pub auth: Authenticator,
    pub browser: Auth,
    pub recovery: Vec<String>,
}

pub async fn set_up(nexus: &Nexus) -> Account {
    let code = nexus.server.setup_code.clone().unwrap();
    let totp: SetupTotp =
        nexus.post("/api/v1/setup", &SetupStart { code: code.clone(), username: "me".into(), password: PASSWORD.into() }, &Auth::None).await;
    let mut auth = Authenticator::new(&totp.secret);
    let answer = nexus
        .post_raw("/api/v1/setup/confirm", &SetupConfirm { code, totp: auth.code().await, device: browser() }, &Auth::None)
        .await;
    assert_eq!(answer.status(), StatusCode::OK);
    let token = session_cookie(&answer).unwrap();
    let signed: SignedIn = answer.json().await.unwrap();
    Account { auth, browser: Auth::Cookie { token, csrf: signed.csrf }, recovery: signed.recovery_codes }
}

/// The session token in an answer's `Set-Cookie`.
pub fn session_cookie(answer: &reqwest::Response) -> Option<String> {
    let value = answer.headers().get("set-cookie")?.to_str().ok()?;
    let token = value.strip_prefix("shed_session=")?.split(';').next()?;
    Some(token.to_string())
}

/// Signs a phone in (with a recovery code, to spare the authenticator's
/// codes); its token.
pub async fn phone_token(nexus: &Nexus, account: &mut Account) -> String {
    let code = account.recovery.pop().expect("a recovery code left");
    let login = Login { username: "me".into(), password: PASSWORD.into(), totp: code, device: phone() };
    let signed: SignedIn = nexus.post("/api/v1/login", &login, &Auth::None).await;
    signed.token.unwrap()
}

/// Pairs a workspace; its id and token.
pub async fn pair(nexus: &Nexus, account: &mut Account, name: &str) -> (String, String) {
    let started: PairStarted =
        nexus.post("/api/v1/pair/start", &PairStart { name: name.into(), version: "0.1.0".into() }, &Auth::None).await;
    let approve = PairApprove { code: started.code, name: name.into(), totp: account.auth.code().await };
    let _: serde_json::Value = nexus.post("/api/v1/pair/approve", &approve, &account.browser).await;
    match nexus.post("/api/v1/pair/poll", &PairPoll { poll: started.poll }, &Auth::None).await {
        PairState::Approved(approved) => (approved.workspace, approved.token),
        other => panic!("not approved: {other:?}"),
    }
}

pub type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens a WebSocket with these headers; the HTTP status if refused.
pub async fn connect(url: &str, headers: &[(&str, String)]) -> Result<Socket, u16> {
    let mut request = url.into_client_request().unwrap();
    for (name, value) in headers {
        let name = tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes()).unwrap();
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(answer)) => Err(answer.status().as_u16()),
        Err(e) => panic!("connecting: {e}"),
    }
}

pub async fn send(socket: &mut Socket, frame: &impl Serialize) {
    send_text(socket, serde_json::to_string(frame).unwrap()).await;
}

pub async fn send_text(socket: &mut Socket, text: String) {
    socket.send(Message::Text(text.into())).await.unwrap();
}

/// The next text frame as text; None when the socket closes.
pub async fn next_text(socket: &mut Socket) -> Option<String> {
    let wait = async {
        loop {
            match socket.next().await? {
                Ok(Message::Text(text)) => return Some(text.to_string()),
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => continue,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await.expect("a frame within 10 s")
}

pub async fn next<T: DeserializeOwned>(socket: &mut Socket) -> T {
    let text = next_text(socket).await.expect("a frame, not the end");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// Frames until one `pick` takes (others are skipped).
pub async fn until<T: DeserializeOwned, R>(socket: &mut Socket, mut pick: impl FnMut(T) -> Option<R>) -> R {
    loop {
        if let Some(found) = pick(next(socket).await) {
            return found;
        }
    }
}

/// Whether the socket ends (after any frames) within a few seconds.
pub async fn closes(socket: &mut Socket) -> bool {
    let wait = async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), wait).await.is_ok()
}

/// A link for a paired workspace, welcomed.
pub async fn link(nexus: &Nexus, token: &str) -> Socket {
    let mut socket = connect(&nexus.ws_url("/link"), &[("authorization", format!("Bearer {token}"))]).await.unwrap();
    send(&mut socket, &LinkFrame::Hello(LinkHello { api: API_VERSION, version: "0.1.0".into(), policy: Policy::Full })).await;
    match next::<NexusFrame>(&mut socket).await {
        NexusFrame::Welcome(_) => socket,
        other => panic!("no welcome: {other:?}"),
    }
}

/// A connector (a phone, by its token), welcomed.
pub async fn connector(nexus: &Nexus, token: &str) -> (Socket, Welcome) {
    let mut socket = connect(&nexus.ws_url("/api"), &[("authorization", format!("Bearer {token}"))]).await.unwrap();
    send(&mut socket, &ClientFrame::Hello(ClientHello { api: API_VERSION, client: "test".into() })).await;
    match next::<ServerFrame>(&mut socket).await {
        ServerFrame::Welcome(welcome) => (socket, welcome),
        other => panic!("no welcome: {other:?}"),
    }
}
