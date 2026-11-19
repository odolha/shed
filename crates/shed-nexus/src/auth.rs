//! Secrets and the checks on them: random tokens and their hashes, password
//! hashing (argon2), TOTP codes, recovery codes, and the limits on failed
//! attempts.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::config::LoginConfig;

/// A new token: 256 random bits, base64url.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// What the database keeps of a token: its SHA-256, in hex.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two secrets in constant time.
pub fn same(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing the password: {e}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else { return false };
    argon2::Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// What passwords must be.
pub fn password_ok(password: &str) -> Result<(), &'static str> {
    if password.chars().count() < 10 {
        return Err("the password needs at least 10 characters");
    }
    if password.len() > 1024 {
        return Err("the password is too long");
    }
    Ok(())
}

/// A new TOTP secret (160 bits), base32.
pub fn new_totp_secret() -> String {
    match Secret::generate_secret().to_encoded() {
        Secret::Encoded(s) => s,
        Secret::Raw(_) => unreachable!("to_encoded gives an encoded secret"),
    }
}

/// The account's authenticator: SHA-1, 6 digits, 30 s steps (what every
/// authenticator app does), labelled `Shed:<username>`.
pub fn totp(secret: &str, username: &str) -> anyhow::Result<TOTP> {
    let bytes = Secret::Encoded(secret.to_string()).to_bytes().map_err(|e| anyhow::anyhow!("TOTP secret: {e:?}"))?;
    let label = username.replace(':', "");
    TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes, Some("Shed".into()), label)
        .map_err(|e| anyhow::anyhow!("TOTP: {e}"))
}

/// The time step a code is valid for, if it is one of the current step or
/// the ones either side of it.
pub fn totp_step(totp: &TOTP, code: &str, now: u64) -> Option<u64> {
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let step = now / 30;
    let mut found = None;
    // Every candidate is compared, so the time taken says nothing.
    for candidate in [step.saturating_sub(1), step, step + 1] {
        if same(&totp.generate(candidate * 30), &code) {
            found = Some(candidate);
        }
    }
    found
}

/// Ten single-use codes, `XXXX-XXXX-XXXX-XXXX` (80 bits each).
pub fn new_recovery_codes() -> Vec<String> {
    (0..10)
        .map(|_| {
            let chars = random_chars(16);
            chars.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect::<Vec<_>>().join("-")
        })
        .collect()
}

/// A recovery code as stored (hashed): upper case, no separators.
pub fn recovery_hash(code: &str) -> String {
    let normal: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    hash_token(&normal)
}

/// Looks like a recovery code rather than a TOTP code.
pub fn is_recovery_code(code: &str) -> bool {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).count() == 16
}

/// Letters and digits that are not easily mixed up (no 0/O, 1/I/L).
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// `n` random characters from `ALPHABET`.
pub fn random_chars(n: usize) -> String {
    let mut rng = rand::rng();
    (0..n)
        .map(|_| {
            // Rejection sampling keeps every character equally likely.
            loop {
                let b = (rng.next_u32() & 0xff) as usize;
                if b < ALPHABET.len() * (256 / ALPHABET.len()) {
                    return ALPHABET[b % ALPHABET.len()] as char;
                }
            }
        })
        .collect()
}

/// Failed attempts (sign-ins, setup codes, one-time codes) per address and
/// in all, and buckets for other unauthenticated calls.
pub struct Guard {
    config: LoginConfig,
    inner: Mutex<GuardInner>,
}

#[derive(Default)]
struct GuardInner {
    failures: HashMap<IpAddr, Failures>,
    global: VecDeque<Instant>,
    buckets: HashMap<(&'static str, IpAddr), Bucket>,
}

#[derive(Default)]
struct Failures {
    times: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Guard {
    pub fn new(config: LoginConfig) -> Guard {
        Guard { config, inner: Mutex::new(GuardInner::default()) }
    }

    fn window(&self) -> Duration {
        Duration::from_secs(self.config.lockout_minutes.max(1) as u64 * 60)
    }

    /// Whether `ip` may try now (not locked out, nor everyone).
    pub fn may_try(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let window = self.window();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while inner.global.front().is_some_and(|t| now.duration_since(*t) > window) {
            inner.global.pop_front();
        }
        if inner.global.len() >= self.config.global_max_failures.max(1) as usize {
            return false;
        }
        match inner.failures.get(&ip).and_then(|f| f.locked_until) {
            Some(until) => until <= now,
            None => true,
        }
    }

    /// Counts a failure from `ip`; locks it out at the limit.
    pub fn failed(&self, ip: IpAddr) {
        let now = Instant::now();
        let window = self.window();
        let max = self.config.max_failures.max(1) as usize;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.global.push_back(now);
        let entry = inner.failures.entry(ip).or_default();
        while entry.times.front().is_some_and(|t| now.duration_since(*t) > window) {
            entry.times.pop_front();
        }
        entry.times.push_back(now);
        if entry.times.len() >= max {
            entry.locked_until = Some(now + window);
            entry.times.clear();
        }
    }

    /// A success from `ip` forgets its failures.
    pub fn succeeded(&self, ip: IpAddr) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.failures.remove(&ip);
    }

    /// Takes one of `per_minute` tokens of `what` for `ip`; false if none is
    /// left.
    pub fn take(&self, what: &'static str, ip: IpAddr, per_minute: u32) -> bool {
        let now = Instant::now();
        let rate = per_minute.max(1) as f64;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = inner.buckets.entry((what, ip)).or_insert(Bucket { tokens: rate, at: now });
        bucket.tokens = (bucket.tokens + now.duration_since(bucket.at).as_secs_f64() * rate / 60.0).min(rate);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Forgets what no longer matters (run now and then).
    pub fn prune(&self) {
        let now = Instant::now();
        let window = self.window();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.failures.retain(|_, f| {
            f.locked_until.is_some_and(|u| u > now) || f.times.back().is_some_and(|t| now.duration_since(*t) < window)
        });
        inner.buckets.retain(|_, b| now.duration_since(b.at) < Duration::from_secs(600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_hashed() {
        let token = random_token();
        assert_eq!(token.len(), 43);
        assert_ne!(random_token(), token);
        assert_eq!(hash_token(&token).len(), 64);
        assert_eq!(hash_token(&token), hash_token(&token));
    }

    #[test]
    fn passwords_verify() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "wrong horse"));
    }

    #[test]
    fn totp_accepts_a_step_either_side() {
        let secret = new_totp_secret();
        let totp = totp(&secret, "me").unwrap();
        let now = 1_000_000_000;
        assert_eq!(totp_step(&totp, &totp.generate(now), now), Some(now / 30));
        assert_eq!(totp_step(&totp, &totp.generate(now - 30), now), Some(now / 30 - 1));
        assert_eq!(totp_step(&totp, &totp.generate(now + 30), now), Some(now / 30 + 1));
        assert_eq!(totp_step(&totp, &totp.generate(now - 90), now), None);
        assert_eq!(totp_step(&totp, "abc", now), None);
    }

    #[test]
    fn recovery_codes_normalise() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert_eq!(codes[0].len(), 19);
        assert!(is_recovery_code(&codes[0]));
        assert_eq!(recovery_hash(&codes[0]), recovery_hash(&codes[0].to_lowercase().replace('-', " ")));
    }

    #[test]
    fn lockout_after_failures() {
        let guard = Guard::new(LoginConfig { max_failures: 3, ..LoginConfig::default() });
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        for _ in 0..3 {
            assert!(guard.may_try(ip));
            guard.failed(ip);
        }
        assert!(!guard.may_try(ip));
        assert!(guard.may_try(other));
    }

    #[test]
    fn buckets_run_out() {
        let guard = Guard::new(LoginConfig::default());
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(guard.take("x", ip, 2));
        assert!(guard.take("x", ip, 2));
        assert!(!guard.take("x", ip, 2));
        assert!(guard.take("y", ip, 2));
    }
}
