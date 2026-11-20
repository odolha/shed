//! Pairing a workspace. `shed nexus join` asks (`pair/start`) and gets a
//! code to show (`K7QF-2M9D`) and a secret to poll with. The signed-in user
//! types the code into the web app (the list of waiting requests does not
//! show codes, so a look-alike request cannot be approved by mistake) and
//! approves it with a fresh TOTP code, naming the workspace. The next poll
//! makes the workspace (a device with a token) and hands the token out,
//! once. Requests are gone after ten minutes.
//!
//! A few requests wait per address (an IPv4 address or IPv6 /64); past the
//! cap on all of them, the oldest make way, so a flood cannot keep a real
//! request out.

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
/// At most this many requests wait at once; a new one past it drops the
/// oldest.
const MAX_PENDING: usize = 50;
/// At most this many from one address.
const MAX_PENDING_PER_ADDRESS: usize = 3;

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
    let waiting = app.store.pending_pairings(now())?;
    let key = auth::address_key(client.ip);
    let from_here = |p: &&Pairing| p.address.parse().is_ok_and(|ip| auth::address_key(ip) == key);
    if waiting.iter().filter(from_here).count() >= MAX_PENDING_PER_ADDRESS {
        return Err(Failure::new(ErrorCode::RateLimited, "too many pairing requests waiting from this address"));
    }
    // Oldest first: they make way.
    for old in waiting.iter().take((waiting.len() + 1).saturating_sub(MAX_PENDING)) {
        app.store.delete_pairing(&old.code)?;
    }
    let mut name = clip(&req.name, 64);
    if name.is_empty() {
        name = "workspace".into();
    }
    let code = normal_code(&auth::random_chars(8));
    let poll = auth::random_token();
    let poll_hash = auth::hash_token(&poll);
    let pairing = Pairing {
        id: crate::store::pairing_id(&poll_hash),
        code: code.clone(),
        name,
        version: clip(&req.version, 32),
        address: client.address(),
        country: client.country.clone(),
        expires: now() + TTL,
        state: PairingState::Pending,
    };
    app.store.add_pairing(&pairing, &poll_hash)?;
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
                // Typed by the person, never shown here.
                code: String::new(),
                id: p.id,
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
    pending_one(app.store.pairing_by_code(&normal_code(code))?)
}

fn pending_one(found: Option<Pairing>) -> Result<Pairing, Failure> {
    match found {
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
    fresh_totp(&app, &caller, &req.totp)?;
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
    let pairing = if req.code.trim().is_empty() {
        pending_one(app.store.pairing_by_id(req.id.trim())?)?
    } else {
        waiting(&app, &req.code)?
    };
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
