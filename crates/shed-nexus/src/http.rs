//! The HTTP side: routes (`shed_api::http` lists them), errors as `ApiError`
//! JSON, who is calling (session cookie with CSRF token, or device token),
//! the address filter, security headers and the web app's files.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequest, FromRequestParts, Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::de::DeserializeOwned;
use shed_api::http::*;
use shed_api::{API_VERSION, ApiError, ErrorCode};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::store::AuditInfo;
use crate::{App, PendingSetup, auth, now, pair, relay};

pub const COOKIE: &str = "shed_session";
pub const CSRF_HEADER: &str = "x-shed-csrf";

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/status", get(status))
        .route("/setup", post(setup))
        .route("/setup/confirm", post(setup_confirm))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/devices", get(devices))
        .route("/devices/revoke", post(revoke))
        .route("/devices/rename", post(rename))
        .route("/password", post(password))
        .route("/pair/start", post(pair::start))
        .route("/pair/poll", post(pair::poll))
        .route("/pair/pending", get(pair::pending))
        .route("/pair/approve", post(pair::approve))
        .route("/pair/reject", post(pair::reject))
        .route("/push", get(push_info))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/prefs", post(push_prefs))
        .route("/audit", get(audit))
        .fallback(|| async { Failure::new(ErrorCode::NotFound, "no such route") })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")));

    let mut router = Router::new()
        .nest("/api/v1", api)
        .route("/link", get(relay::link))
        .route("/api", get(relay::api));
    router = match &app.config.web_dir {
        Some(dir) => router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")))),
        None => router.fallback(|| async { Failure::new(ErrorCode::NotFound, "no web app here") }),
    };

    let mut router = router
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::REFERRER_POLICY, "no-referrer"))
        .layer(header_layer(header::X_FRAME_OPTIONS, "DENY"))
        .layer(header_layer(HeaderName::from_static("cross-origin-opener-policy"), "same-origin"))
        .layer(header_layer(HeaderName::from_static("permissions-policy"), "geolocation=(), microphone=(), usb=()"));
    if let Ok(csp) = HeaderValue::from_str(&app.config.content_security_policy) {
        router = router.layer(SetResponseHeaderLayer::if_not_present(header::CONTENT_SECURITY_POLICY, csp));
    }
    if app.config.public_url.starts_with("https://") {
        router = router.layer(header_layer(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000"));
    }
    router.with_state(app)
}

fn header_layer(name: HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

// ------------------------------------------------------------------ errors

/// An `ApiError` as an HTTP answer, with the status its code means.
#[derive(Debug)]
pub struct Failure(pub ApiError);

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Failure {
        Failure(ApiError::new(code, message))
    }
}

pub fn status_of(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden | ErrorCode::WrongCode => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Offline => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Invalid | ErrorCode::Unsupported => StatusCode::BAD_REQUEST,
        ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::Failed => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (status_of(self.0.code), Json(self.0)).into_response()
    }
}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Failure {
        tracing::error!("{e:#}");
        Failure::new(ErrorCode::Failed, "something failed in the nexus")
    }
}

pub type Answer<T> = Result<Json<T>, Failure>;

/// The empty answer, `{}`.
#[derive(serde::Serialize)]
pub struct Empty {}

/// A JSON body; one that does not read is `Invalid`.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = Failure;

    async fn from_request(req: Request, state: &S) -> Result<Self, Failure> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Body(value)),
            Err(e) => Err(Failure::new(ErrorCode::Invalid, e.body_text())),
        }
    }
}

// ------------------------------------------------------------ who's asking

/// Where a request comes from: its address (from the proxy, if trusted) and
/// country, if known.
#[derive(Clone, Debug)]
pub struct Client {
    pub ip: IpAddr,
    pub country: Option<String>,
}

impl Client {
    pub fn address(&self) -> String {
        crate::geo::canonical(self.ip).to_string()
    }

    pub fn audit<'a>(&'a self, address: &'a str, device: Option<&'a str>, detail: &'a str) -> AuditInfo<'a> {
        AuditInfo { device, address, country: self.country.as_deref(), detail }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Failure> {
        parts.extensions.get::<Client>().cloned().ok_or_else(|| Failure::new(ErrorCode::Failed, "no client address"))
    }
}

/// Finds the client's address and keeps out those the filter refuses.
async fn gate(State(app): State<Arc<App>>, mut req: Request, next: Next) -> Response {
    let Some(peer) = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip()) else {
        return Failure::new(ErrorCode::Failed, "no client address").into_response();
    };
    let ip = client_ip(peer, req.headers(), &app.proxies);
    let country = app.filter.country(ip);
    if !app.filter.allows(ip, country.as_deref()) {
        tracing::info!("refused {ip} ({})", country.as_deref().unwrap_or("unknown country"));
        return Failure::new(ErrorCode::Forbidden, "not from here").into_response();
    }
    req.extensions_mut().insert(Client { ip, country });
    next.run(req).await
}

/// The client's address: the TCP peer, unless that is a trusted proxy;
/// then, walking `X-Forwarded-For` from the right (each proxy appends the
/// address it heard from), the first address that is not a trusted proxy.
/// What a client wrote at the left of the header is never believed past an
/// untrusted hop.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, proxies: &[ipnet::IpNet]) -> IpAddr {
    let trusted = |ip: IpAddr| proxies.iter().any(|n| n.contains(&crate::geo::canonical(ip)));
    let peer = crate::geo::canonical(peer);
    if !trusted(peer) {
        return peer;
    }
    let hops: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .collect();
    let mut client = peer;
    for hop in hops.iter().rev() {
        let Ok(ip) = hop.parse::<IpAddr>() else { break };
        client = crate::geo::canonical(ip);
        if !trusted(client) {
            break;
        }
    }
    client
}

/// A signed-in device: a browser by its session cookie, or a phone by its
/// device token.
#[derive(Clone, Debug)]
pub struct Caller {
    pub device: String,
    /// The session's token hash, for a browser.
    pub session: Option<String>,
}

impl FromRequestParts<Arc<App>> for Caller {
    type Rejection = Failure;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Failure> {
        let client = Client::from_request_parts(parts, app).await?;
        let caller = authenticate(app, &parts.headers, &parts.method)?;
        app.store.touch_device(&caller.device, &client.address(), client.country.as_deref(), now())?;
        app.guard.remember(client.ip);
        Ok(caller)
    }
}

/// Who `headers` sign in as; for a cookie session, a change (any method but
/// GET and HEAD) needs the CSRF token too.
pub fn authenticate(app: &App, headers: &HeaderMap, method: &Method) -> Result<Caller, Failure> {
    let unauthorized = || Failure::new(ErrorCode::Unauthorized, "not signed in");
    if let Some(token) = bearer(headers) {
        let device = app.store.device_by_token(&auth::hash_token(token))?.ok_or_else(unauthorized)?;
        if device.kind != DeviceKind::Phone {
            return Err(unauthorized());
        }
        return Ok(Caller { device: device.id, session: None });
    }
    let token = cookie(headers, COOKIE).ok_or_else(unauthorized)?;
    let hash = auth::hash_token(token);
    let session = app.store.session(&hash, now())?.ok_or_else(unauthorized)?;
    if method != Method::GET && method != Method::HEAD {
        let sent = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();
        if !auth::same(sent, &session.csrf) {
            return Err(Failure::new(ErrorCode::Forbidden, "missing or wrong CSRF token"));
        }
    }
    Ok(Caller { device: session.device, session: Some(hash) })
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))?.trim();
    (!token.is_empty()).then_some(token)
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

fn session_cookie(app: &App, token: &str, max_age: u64) -> HeaderValue {
    let secure = if app.config.secure_cookies { "; Secure" } else { "" };
    let value = format!("{COOKIE}={token}; HttpOnly{secure}; SameSite=Strict; Path=/; Max-Age={max_age}");
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// The answer to an attempt the guard holds back.
pub fn refused(why: auth::Refused) -> Failure {
    match why {
        auth::Refused::Locked => Failure::new(ErrorCode::RateLimited, "too many attempts; try again later"),
        auth::Refused::Busy => {
            Failure::new(ErrorCode::RateLimited, "the nexus sees many failed sign-ins; try again in a minute")
        }
    }
}

/// Starts an anonymous attempt from `client` (counted until it succeeds).
async fn begin(app: &App, client: &Client) -> Result<(), Failure> {
    app.guard.begin(client.ip).await.map_err(refused)
}

/// Runs argon2 (hashing or checking a password) off the async threads, a
/// few at a time: each takes ~19 MiB and a core, so attempts at once must
/// not add up. Waits a while for a turn, then answers `RateLimited`.
pub async fn argon<T: Send + 'static>(app: &App, work: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
    let permit = tokio::time::timeout(ARGON_WAIT, app.argon.clone().acquire_owned())
        .await
        .map_err(|_| Failure::new(ErrorCode::RateLimited, "the nexus is busy; try again"))?
        .map_err(|e| anyhow::anyhow!(e))?;
    // The permit goes with the work: it is held until argon2 is done, even
    // if the request is dropped.
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))?;
    Ok(result)
}

/// How long an argon2 job waits for its turn.
const ARGON_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `code` is the authenticator's, not used yet (and uses it).
fn totp_right(app: &App, code: &str) -> Result<bool, Failure> {
    let account = app.store.account()?.ok_or_else(|| Failure::new(ErrorCode::Invalid, "no account"))?;
    let totp = auth::totp(&account.totp_secret, &account.username)?;
    Ok(match auth::totp_step(&totp, code, now()) {
        Some(step) => app.store.use_totp_step(step)?,
        None => false,
    })
}

/// Checks a fresh TOTP code from a signed-in device (for revoking, pairing,
/// the password). Wrong codes are counted per device, apart from sign-ins,
/// so failed sign-ins from elsewhere never block these.
pub fn fresh_totp(app: &App, caller: &Caller, code: &str) -> Result<(), Failure> {
    app.guard.begin_fresh(&caller.device).map_err(|_| {
        Failure::new(ErrorCode::RateLimited, "too many wrong codes from this device; try again later")
    })?;
    if totp_right(app, code)? {
        app.guard.fresh_ok(&caller.device);
        return Ok(());
    }
    Err(Failure::new(ErrorCode::WrongCode, "wrong or already used code"))
}

fn valid_name(name: &str) -> Result<String, Failure> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(Failure::new(ErrorCode::Invalid, "a name needs 1 to 64 characters"));
    }
    Ok(name.to_string())
}

/// A signed-in answer, with the session cookie for a browser.
struct SignedInAnswer {
    body: SignedIn,
    cookie: Option<HeaderValue>,
}

impl IntoResponse for SignedInAnswer {
    fn into_response(self) -> Response {
        let mut response = Json(self.body).into_response();
        if let Some(cookie) = self.cookie {
            response.headers_mut().insert(header::SET_COOKIE, cookie);
        }
        response
    }
}

/// Signs `device` in: a browser gets a session (and the cookie), a phone a
/// device token.
fn sign_in(app: &App, client: &Client, username: &str, device: &DeviceHello) -> Result<SignedInAnswer, Failure> {
    let name = valid_name(&device.name).unwrap_or_else(|_| "unnamed".to_string());
    let address = client.address();
    let country = client.country.as_deref();
    let mut body =
        SignedIn { username: username.to_string(), device: String::new(), token: None, csrf: None, recovery_codes: vec![] };
    match device.kind {
        DeviceKind::Browser => {
            body.device = app.store.add_device(DeviceKind::Browser, &name, None, &address, country, now())?;
            let token = auth::random_token();
            let csrf = auth::random_token();
            let lifetime = app.config.session_days as u64 * 86400;
            app.store.add_session(&auth::hash_token(&token), &csrf, &body.device, now() + lifetime)?;
            body.csrf = Some(csrf);
            Ok(SignedInAnswer { body, cookie: Some(session_cookie(app, &token, lifetime)) })
        }
        DeviceKind::Phone => {
            let token = auth::random_token();
            let hash = auth::hash_token(&token);
            body.device = app.store.add_device(DeviceKind::Phone, &name, Some(&hash), &address, country, now())?;
            body.token = Some(token);
            Ok(SignedInAnswer { body, cookie: None })
        }
        DeviceKind::Workspace => Err(Failure::new(ErrorCode::Invalid, "workspaces pair, they do not sign in")),
    }
}

// ------------------------------------------------------------------ routes

async fn status(State(app): State<Arc<App>>, headers: HeaderMap) -> Answer<NexusStatus> {
    Ok(Json(NexusStatus {
        version: crate::VERSION.into(),
        api: API_VERSION,
        ready: app.store.account()?.is_some(),
        signed_in: authenticate(&app, &headers, &Method::GET).is_ok(),
    }))
}

/// Checks the setup code; the attempt counts as a failure until the
/// account is made.
async fn check_setup_code(app: &App, client: &Client, code: &str) -> Result<(), Failure> {
    begin(app, client).await?;
    if app.store.account()?.is_some() {
        return Err(Failure::new(ErrorCode::Forbidden, "the account is set up already"));
    }
    let setup = app.setup.lock().unwrap_or_else(|e| e.into_inner());
    if setup.code.as_deref().is_some_and(|expected| auth::same(code.trim(), expected)) {
        return Ok(());
    }
    Err(Failure::new(ErrorCode::Unauthorized, "wrong setup code (see the setup link's file, named in the nexus's log)"))
}

async fn setup(State(app): State<Arc<App>>, client: Client, Body(req): Body<SetupStart>) -> Answer<SetupTotp> {
    check_setup_code(&app, &client, &req.code).await?;
    let username = valid_name(&req.username)?;
    if username.contains(':') {
        return Err(Failure::new(ErrorCode::Invalid, "a user name cannot have ':' in it"));
    }
    auth::password_ok(&req.password).map_err(|e| Failure::new(ErrorCode::Invalid, e))?;
    let password = req.password;
    let hash = argon(&app, move || auth::hash_password(&password)).await??;
    let secret = auth::new_totp_secret();
    let totp = auth::totp(&secret, &username)?;
    let answer = SetupTotp { secret: secret.clone(), url: totp.get_url() };
    app.setup.lock().unwrap_or_else(|e| e.into_inner()).pending =
        Some(PendingSetup { username, password: hash, totp_secret: secret });
    Ok(Json(answer))
}

async fn setup_confirm(
    State(app): State<Arc<App>>,
    client: Client,
    Body(req): Body<SetupConfirm>,
) -> Result<SignedInAnswer, Failure> {
    check_setup_code(&app, &client, &req.code).await?;
    if req.device.kind == DeviceKind::Workspace {
        return Err(Failure::new(ErrorCode::Invalid, "workspaces pair, they do not sign in"));
    }
    let (username, password, secret) = {
        let setup = app.setup.lock().unwrap_or_else(|e| e.into_inner());
        let pending = setup.pending.as_ref().ok_or_else(|| Failure::new(ErrorCode::Invalid, "start the setup first"))?;
        (pending.username.clone(), pending.password.clone(), pending.totp_secret.clone())
    };
    let totp = auth::totp(&secret, &username)?;
    let Some(step) = auth::totp_step(&totp, &req.totp, now()) else {
        return Err(Failure::new(ErrorCode::WrongCode, "wrong code from the authenticator"));
    };
    let codes = auth::new_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|c| auth::recovery_hash(c)).collect();
    let account = crate::store::Account { username: username.clone(), password, totp_secret: secret, totp_step: step };
    app.store.create_account(&account, &hashes, now())?;
    *app.setup.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
    let _ = std::fs::remove_file(app.config.data_dir().join(crate::SETUP_FILE));
    app.guard.succeeded(client.ip);
    let address = client.address();
    app.store.audit("setup", client.audit(&address, None, ""), now())?;
    tracing::info!("the account is set up");
    let mut answer = sign_in(&app, &client, &username, &req.device)?;
    answer.body.recovery_codes = codes;
    Ok(answer)
}

async fn login(State(app): State<Arc<App>>, client: Client, Body(req): Body<Login>) -> Result<SignedInAnswer, Failure> {
    if req.device.kind == DeviceKind::Workspace {
        return Err(Failure::new(ErrorCode::Invalid, "workspaces pair, they do not sign in"));
    }
    if req.password.len() > 1024 {
        return Err(Failure::new(ErrorCode::Invalid, "the password is too long"));
    }
    let account = app.store.account()?.ok_or_else(|| Failure::new(ErrorCode::Unauthorized, "not set up yet"))?;
    // Counted as a failure from here until it succeeds, so attempts made at
    // once count before any of them is checked.
    begin(&app, &client).await?;
    let address = client.address();
    let wrong = |detail: &str| -> Result<SignedInAnswer, Failure> {
        app.store.audit("login-failed", client.audit(&address, None, detail), now())?;
        Err(Failure::new(ErrorCode::Unauthorized, "wrong user name, password or code"))
    };

    // The password is always checked, so the time taken does not say whether
    // the user name was right.
    let hash = account.password.clone();
    let password = req.password.clone();
    let password_ok = argon(&app, move || auth::verify_password(&hash, &password)).await?;
    let user_ok = auth::same(req.username.trim(), &account.username);
    if !(password_ok && user_ok) {
        return wrong("wrong user name or password");
    }
    let mut detail = "";
    let second = if auth::is_recovery_code(&req.totp) {
        detail = "with a recovery code";
        app.store.use_recovery_code(&auth::recovery_hash(&req.totp), now())?
    } else {
        let totp = auth::totp(&account.totp_secret, &account.username)?;
        match auth::totp_step(&totp, &req.totp, now()) {
            Some(step) => app.store.use_totp_step(step)?,
            None => false,
        }
    };
    if !second {
        return wrong("wrong or reused code");
    }
    app.guard.succeeded(client.ip);
    let answer = sign_in(&app, &client, &account.username, &req.device)?;
    app.store.audit("login", client.audit(&address, Some(&answer.body.device), detail), now())?;
    Ok(answer)
}

async fn logout(State(app): State<Arc<App>>, client: Client, caller: Caller) -> Result<Response, Failure> {
    app.store.revoke_device(&caller.device)?;
    app.hub.close_device(&caller.device, ApiError::new(ErrorCode::Unauthorized, "signed out"));
    let address = client.address();
    app.store.audit("logout", client.audit(&address, Some(&caller.device), ""), now())?;
    let mut response = Json(Empty {}).into_response();
    if caller.session.is_some() {
        response.headers_mut().insert(header::SET_COOKIE, session_cookie(&app, "", 0));
    }
    Ok(response)
}

async fn devices(State(app): State<Arc<App>>, caller: Caller) -> Answer<Vec<Device>> {
    let online = app.hub.connected_devices();
    let rows = app.store.devices(now())?;
    Ok(Json(
        rows.into_iter()
            .map(|d| Device {
                current: d.id == caller.device,
                online: online.contains(&d.id),
                id: d.id,
                kind: d.kind,
                name: d.name,
                created: d.created as u32,
                seen: d.seen as u32,
                address: d.address,
                country: d.country,
            })
            .collect(),
    ))
}

async fn revoke(State(app): State<Arc<App>>, client: Client, caller: Caller, Body(req): Body<Revoke>) -> Answer<Empty> {
    fresh_totp(&app, &caller, &req.totp)?;
    let device = app.store.device(&req.device)?.filter(|d| !d.revoked);
    let Some(device) = device else { return Err(Failure::new(ErrorCode::NotFound, "no such device")) };
    app.store.revoke_device(&device.id)?;
    app.hub.close_device(&device.id, ApiError::new(ErrorCode::Unauthorized, "this device was revoked"));
    let address = client.address();
    let detail = format!("{} ({})", device.name, crate::store::kind_str(device.kind));
    app.store.audit("revoke", client.audit(&address, Some(&caller.device), &detail), now())?;
    if device.kind == DeviceKind::Workspace {
        app.hub.forget(&device.id);
        relay::announce(&app);
    }
    Ok(Json(Empty {}))
}

async fn rename(
    State(app): State<Arc<App>>,
    client: Client,
    caller: Caller,
    Body(req): Body<RenameDevice>,
) -> Answer<Empty> {
    let name = valid_name(&req.name)?;
    let device = app.store.device(&req.device)?.filter(|d| !d.revoked);
    let Some(device) = device else { return Err(Failure::new(ErrorCode::NotFound, "no such device")) };
    app.store.rename_device(&device.id, &name)?;
    let address = client.address();
    app.store.audit("rename", client.audit(&address, Some(&caller.device), &name), now())?;
    if device.kind == DeviceKind::Workspace {
        relay::announce(&app);
    }
    Ok(Json(Empty {}))
}

async fn password(
    State(app): State<Arc<App>>,
    client: Client,
    caller: Caller,
    Body(req): Body<ChangePassword>,
) -> Answer<Empty> {
    auth::password_ok(&req.new).map_err(|e| Failure::new(ErrorCode::Invalid, e))?;
    if req.old.len() > 1024 {
        return Err(Failure::new(ErrorCode::Forbidden, "wrong password"));
    }
    // The old password and the code count as one attempt of this device's.
    app.guard.begin_fresh(&caller.device).map_err(|_| {
        Failure::new(ErrorCode::RateLimited, "too many wrong attempts from this device; try again later")
    })?;
    let account = app.store.account()?.ok_or_else(|| Failure::new(ErrorCode::Invalid, "no account"))?;
    let (hash, old) = (account.password.clone(), req.old.clone());
    let old_ok = argon(&app, move || auth::verify_password(&hash, &old)).await?;
    if !old_ok {
        return Err(Failure::new(ErrorCode::Forbidden, "wrong password"));
    }
    if !totp_right(&app, &req.totp)? {
        return Err(Failure::new(ErrorCode::WrongCode, "wrong or already used code"));
    }
    app.guard.fresh_ok(&caller.device);
    let new = req.new;
    let hash = argon(&app, move || auth::hash_password(&new)).await??;
    app.store.set_password(&hash)?;
    for id in app.store.revoke_people_devices_but(&caller.device)? {
        app.hub.close_device(&id, ApiError::new(ErrorCode::Unauthorized, "the password changed"));
    }
    let address = client.address();
    app.store.audit("password", client.audit(&address, Some(&caller.device), ""), now())?;
    Ok(Json(Empty {}))
}

async fn push_info(State(app): State<Arc<App>>, caller: Caller) -> Answer<PushInfo> {
    Ok(Json(PushInfo {
        vapid_key: app.push.as_ref().map(|p| p.public_key().to_string()),
        prefs: PushPrefs { kinds: app.store.push_kinds(&caller.device)? },
    }))
}

async fn push_subscribe(
    State(app): State<Arc<App>>,
    caller: Caller,
    Body(req): Body<PushSubscribe>,
) -> Answer<Empty> {
    let Some(push) = &app.push else { return Err(Failure::new(ErrorCode::Unsupported, "push is off on this nexus")) };
    push.check(&req).map_err(|e| Failure::new(ErrorCode::Invalid, e))?;
    push.check_address(&req.endpoint).await.map_err(|e| Failure::new(ErrorCode::Invalid, e))?;
    app.store.set_subscription(&caller.device, &req)?;
    Ok(Json(Empty {}))
}

async fn push_prefs(State(app): State<Arc<App>>, caller: Caller, Body(req): Body<PushPrefs>) -> Answer<Empty> {
    app.store.set_push_kinds(&caller.device, &req.kinds)?;
    Ok(Json(Empty {}))
}

async fn audit(State(app): State<Arc<App>>, _caller: Caller) -> Answer<Vec<AuditEntry>> {
    Ok(Json(app.store.audit_entries(500)?))
}
