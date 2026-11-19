//! Pairing a workspace. `shed nexus join` asks (`pair/start`) and gets a
//! code to show (`K7QF-2M9D`) and a secret to poll with. The signed-in user
//! compares the code and approves it with a fresh TOTP code, naming the
//! workspace. The next poll makes the workspace (a device with a token) and
//! hands the token out, once. Requests are gone after ten minutes.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use shed_api::ErrorCode;
use shed_api::http::{PairApprove, PairApproved, PairPoll, PairReject, PairStart, PairStarted, PairState, PendingPairing};

use crate::http::{Answer, Body, Caller, Client, Empty, Failure, fresh_totp};
use crate::store::{Pairing, PairingState};
use crate::{App, auth, now, relay};

/// How long a request waits for approval, and then for its poll.
const TTL: u64 = 600;
/// At most this many requests wait at once.
const MAX_PENDING: usize = 20;

/// `k7qf 2m9d` → `K7QF-2M9D`.
pub fn normal_code(code: &str) -> String {
    let chars: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if chars.len() == 8 { format!("{}-{}", &chars[..4], &chars[4..]) } else { chars }
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

pub async fn start(State(app): State<Arc<App>>, client: Client, Body(req): Body<PairStart>) -> Answer<PairStarted> {
    if !app.guard.take("pair", client.ip, app.config.login.pair_per_minute) {
        return Err(Failure::new(ErrorCode::RateLimited, "too many pairing requests; try again later"));
    }
    if app.store.account()?.is_none() {
        return Err(Failure::new(ErrorCode::Invalid, "the nexus is not set up yet"));
    }
    if app.store.pending_pairings(now())?.len() >= MAX_PENDING {
        return Err(Failure::new(ErrorCode::RateLimited, "too many pairing requests waiting"));
    }
    let mut name = clip(&req.name, 64);
    if name.is_empty() {
        name = "workspace".into();
    }
    let code = normal_code(&auth::random_chars(8));
    let poll = auth::random_token();
    let pairing = Pairing {
        code: code.clone(),
        name,
        version: clip(&req.version, 32),
        address: client.address(),
        country: client.country.clone(),
        expires: now() + TTL,
        state: PairingState::Pending,
    };
    app.store.add_pairing(&pairing, &auth::hash_token(&poll))?;
    let address = client.address();
    app.store.audit("pair-request", client.audit(&address, None, &pairing.name), now())?;
    Ok(Json(PairStarted { code, poll, expires: pairing.expires as u32 }))
}

pub async fn poll(State(app): State<Arc<App>>, client: Client, Body(req): Body<PairPoll>) -> Answer<PairState> {
    if !app.guard.take("pair-poll", client.ip, 120) {
        return Err(Failure::new(ErrorCode::RateLimited, "polling too fast"));
    }
    let Some(pairing) = app.store.pairing_by_poll(&auth::hash_token(&req.poll))? else {
        return Ok(Json(PairState::Expired));
    };
    if pairing.expires <= now() {
        app.store.delete_pairing(&pairing.code)?;
        return Ok(Json(PairState::Expired));
    }
    match &pairing.state {
        PairingState::Pending => Ok(Json(PairState::Pending)),
        PairingState::Rejected => {
            app.store.delete_pairing(&pairing.code)?;
            Ok(Json(PairState::Rejected))
        }
        PairingState::Approved(name) => {
            let token = auth::random_token();
            let Some(id) = app.store.complete_pairing(&pairing, name, &auth::hash_token(&token), now())? else {
                return Ok(Json(PairState::Expired));
            };
            let address = client.address();
            app.store.audit("paired", client.audit(&address, Some(&id), name), now())?;
            tracing::info!("paired a workspace");
            relay::announce(&app);
            Ok(Json(PairState::Approved(PairApproved { workspace: id, name: name.clone(), token })))
        }
    }
}

pub async fn pending(State(app): State<Arc<App>>, _caller: Caller) -> Answer<Vec<PendingPairing>> {
    Ok(Json(
        app.store
            .pending_pairings(now())?
            .into_iter()
            .map(|p| PendingPairing {
                code: p.code,
                name: p.name,
                version: p.version,
                address: p.address,
                country: p.country,
                expires: p.expires as u32,
            })
            .collect(),
    ))
}

/// A request still waiting, by its code.
fn waiting(app: &App, code: &str) -> Result<Pairing, Failure> {
    match app.store.pairing_by_code(&normal_code(code))? {
        Some(p) if p.state == PairingState::Pending && p.expires > now() => Ok(p),
        _ => Err(Failure::new(ErrorCode::NotFound, "no such pairing request (it may have expired)")),
    }
}

pub async fn approve(
    State(app): State<Arc<App>>,
    client: Client,
    caller: Caller,
    Body(req): Body<PairApprove>,
) -> Answer<Empty> {
    fresh_totp(&app, &client, &req.totp)?;
    let pairing = waiting(&app, &req.code)?;
    let mut name = clip(&req.name, 64);
    if name.is_empty() {
        name = pairing.name.clone();
    }
    if !app.store.decide_pairing(&pairing.code, &PairingState::Approved(name.clone()), now() + TTL)? {
        return Err(Failure::new(ErrorCode::NotFound, "no such pairing request"));
    }
    let address = client.address();
    app.store.audit("pair-approve", client.audit(&address, Some(&caller.device), &name), now())?;
    Ok(Json(Empty {}))
}

pub async fn reject(
    State(app): State<Arc<App>>,
    client: Client,
    caller: Caller,
    Body(req): Body<PairReject>,
) -> Answer<Empty> {
    let pairing = waiting(&app, &req.code)?;
    if !app.store.decide_pairing(&pairing.code, &PairingState::Rejected, pairing.expires)? {
        return Err(Failure::new(ErrorCode::NotFound, "no such pairing request"));
    }
    let address = client.address();
    app.store.audit("pair-reject", client.audit(&address, Some(&caller.device), &pairing.name), now())?;
    Ok(Json(Empty {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_normalise() {
        assert_eq!(normal_code("k7qf 2m9d"), "K7QF-2M9D");
        assert_eq!(normal_code("K7QF-2M9D"), "K7QF-2M9D");
        assert_eq!(normal_code("ab"), "AB");
    }
}
