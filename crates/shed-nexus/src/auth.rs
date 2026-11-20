//! Secrets and the checks on them: random tokens and their hashes, password
//! hashing (argon2), TOTP codes, recovery codes, and the limits on failed
//! attempts.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};
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

/// The address limits count by: an IPv4 address, or an IPv6 address's /64
/// (one household or server gets a whole /64, so counting single IPv6
/// addresses would count nothing).
pub fn address_key(ip: IpAddr) -> IpAddr {
    match crate::geo::canonical(ip) {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let bits = u128::from(v6) & !((1u128 << 64) - 1);
            IpAddr::V6(Ipv6Addr::from(bits))
        }
    }
}

/// Why an attempt may not go ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// This address (or device) failed too often: locked out for a while.
    Locked,
    /// Too many failures everywhere, and this address's turn is too far off.
    Busy,
}

/// Entries the guard keeps at most per map; the oldest go first.
const MAX_ENTRIES: usize = 10_000;
/// The longest an attempt is held back for its turn while the nexus is
/// slowed down; further off, it is refused.
const MAX_WAIT: Duration = Duration::from_secs(10);
/// Addresses that signed in are remembered this long.
const KNOWN_FOR: Duration = Duration::from_secs(90 * 86400);

/// Limits on attempts to sign in (and on other unauthenticated calls), and,
/// apart, on one-time codes asked of signed-in devices.
///
/// - An anonymous attempt (`begin`) counts as a failure from the moment it
///   starts until it succeeds, so attempts running at once count too. An
///   address with `max_failures` in `lockout_minutes` is locked out.
/// - With `global_max_failures` failures from everywhere, the nexus slows
///   down instead of locking everyone out: addresses that never signed in
///   share `slow_per_minute` attempts, each waiting its turn (up to
///   `MAX_WAIT`); addresses that signed in before are not held back.
/// - One-time codes from signed-in devices (`begin_fresh`) are counted per
///   device, so an attack on the sign-in does not keep the owner from
///   revoking a device or approving a pairing.
pub struct Guard {
    config: LoginConfig,
    inner: Mutex<GuardInner>,
}

#[derive(Default)]
struct GuardInner {
    failures: HashMap<IpAddr, Failures>,
    fresh: HashMap<String, Failures>,
    global: VecDeque<Instant>,
    /// Addresses that signed in, and when last.
    known: HashMap<IpAddr, Instant>,
    /// The shared pace while slowed down.
    slow: Option<Bucket>,
    buckets: HashMap<(&'static str, String), Bucket>,
}

#[derive(Default)]
struct Failures {
    times: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

impl Failures {
    fn last(&self) -> Option<Instant> {
        self.times.back().copied().max(self.locked_until)
    }

    fn locked(&self, now: Instant) -> bool {
        self.locked_until.is_some_and(|u| u > now)
    }

    /// Counts one at `now`; locks at `max` within `window`.
    fn count(&mut self, now: Instant, window: Duration, max: usize) {
        while self.times.front().is_some_and(|t| now.duration_since(*t) > window) {
            self.times.pop_front();
        }
        self.times.push_back(now);
        if self.times.len() >= max {
            self.locked_until = Some(now + window);
            self.times.clear();
        }
    }
}

/// A token bucket: `rate` per minute, up to `burst` saved up.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(burst: f64, now: Instant) -> Bucket {
        Bucket { tokens: burst, at: now }
    }

    fn refill(&mut self, now: Instant, per_minute: f64, burst: f64) {
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * per_minute / 60.0).min(burst);
        self.at = now;
    }
}

/// Keeps `map` under `MAX_ENTRIES` by dropping its oldest entries.
fn bound<K: Clone + Eq + std::hash::Hash, V>(map: &mut HashMap<K, V>, age: impl Fn(&V) -> Option<Instant>) {
    if map.len() < MAX_ENTRIES {
        return;
    }
    let mut ages: Vec<(Option<Instant>, K)> = map.iter().map(|(k, v)| (age(v), k.clone())).collect();
    ages.sort_by_key(|(at, _)| *at);
    for (_, k) in ages.into_iter().take(MAX_ENTRIES / 10) {
        map.remove(&k);
    }
}

impl Guard {
    pub fn new(config: LoginConfig) -> Guard {
        Guard { config, inner: Mutex::new(GuardInner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GuardInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn window(&self) -> Duration {
        Duration::from_secs(self.config.lockout_minutes.max(1) as u64 * 60)
    }

    fn max(&self) -> usize {
        self.config.max_failures.max(1) as usize
    }

    /// Remembers an address that signed in (at start, those devices were
    /// last seen from), so a slowed-down nexus does not hold it back.
    pub fn remember(&self, ip: IpAddr) {
        let mut inner = self.lock();
        bound(&mut inner.known, |at| Some(*at));
        inner.known.insert(address_key(ip), Instant::now());
    }

    /// Starts an anonymous attempt from `ip` (a sign-in, a setup code): it
    /// counts as a failure until `succeeded`. While the nexus is slowed
    /// down, waits for its turn first.
    pub async fn begin(&self, ip: IpAddr) -> Result<(), Refused> {
        let wait = self.reserve(ip, Instant::now())?;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        Ok(())
    }

    /// `begin` without the waiting: how long to wait, the attempt counted.
    fn reserve(&self, ip: IpAddr, now: Instant) -> Result<Duration, Refused> {
        let key = address_key(ip);
        let window = self.window();
        let max = self.max();
        let mut inner = self.lock();
        let inner = &mut *inner;
        if inner.failures.get(&key).is_some_and(|f| f.locked(now)) {
            return Err(Refused::Locked);
        }
        while inner.global.front().is_some_and(|t| now.duration_since(*t) > window) {
            inner.global.pop_front();
        }
        let slowed = inner.global.len() >= self.config.global_max_failures.max(1) as usize;
        let known = inner.known.get(&key).is_some_and(|at| now.duration_since(*at) < KNOWN_FOR);
        let mut wait = Duration::ZERO;
        if slowed && !known {
            let rate = self.config.slow_per_minute.max(1) as f64;
            let slow = inner.slow.get_or_insert_with(|| Bucket::full(1.0, now));
            slow.refill(now, rate, 1.0);
            if slow.tokens < 1.0 {
                wait = Duration::from_secs_f64((1.0 - slow.tokens) * 60.0 / rate);
                if wait > MAX_WAIT {
                    return Err(Refused::Busy);
                }
            }
            // Taken now (the bucket may go below zero): later attempts wait
            // behind this one.
            slow.tokens -= 1.0;
        }
        if !inner.failures.contains_key(&key) {
            bound(&mut inner.failures, Failures::last);
        }
        inner.failures.entry(key).or_default().count(now, window, max);
        inner.global.push_back(now);
        // Only whether the threshold is reached matters.
        let cap = (self.config.global_max_failures.max(1) as usize).saturating_mul(2).max(16);
        while inner.global.len() > cap {
            inner.global.pop_front();
        }
        Ok(wait)
    }

    /// The attempt from `ip` succeeded: its failures are forgotten, and the
    /// address remembered as one that signs in.
    pub fn succeeded(&self, ip: IpAddr) {
        let key = address_key(ip);
        let mut inner = self.lock();
        inner.failures.remove(&key);
        inner.global.pop_back();
        bound(&mut inner.known, |at| Some(*at));
        inner.known.insert(key, Instant::now());
    }

    /// Starts checking a one-time code from a signed-in `device` (counted
    /// as a failure until `fresh_ok`); refused if it failed too often.
    pub fn begin_fresh(&self, device: &str) -> Result<(), Refused> {
        let now = Instant::now();
        let (window, max) = (self.window(), self.max());
        let mut inner = self.lock();
        if inner.fresh.get(device).is_some_and(|f| f.locked(now)) {
            return Err(Refused::Locked);
        }
        if !inner.fresh.contains_key(device) {
            bound(&mut inner.fresh, Failures::last);
        }
        inner.fresh.entry(device.to_string()).or_default().count(now, window, max);
        Ok(())
    }

    /// The device's code was right: its failures are forgotten.
    pub fn fresh_ok(&self, device: &str) {
        self.lock().fresh.remove(device);
    }

    /// Takes one of `per_minute` tokens of `what` for `ip`'s address; false
    /// if none is left.
    pub fn take(&self, what: &'static str, ip: IpAddr, per_minute: u32) -> bool {
        self.take_for(what, &address_key(ip).to_string(), per_minute, per_minute)
    }

    /// Takes a token of `what` for `key`: `per_minute`, at most `burst` at
    /// once; false if none is left.
    pub fn take_for(&self, what: &'static str, key: &str, per_minute: u32, burst: u32) -> bool {
        let now = Instant::now();
        let (rate, burst) = (per_minute.max(1) as f64, burst.max(1) as f64);
        let mut inner = self.lock();
        let id = (what, key.to_string());
        if !inner.buckets.contains_key(&id) {
            bound(&mut inner.buckets, |b| Some(b.at));
        }
        let bucket = inner.buckets.entry(id).or_insert_with(|| Bucket::full(burst, now));
        bucket.refill(now, rate, burst);
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
        let mut inner = self.lock();
        let keep = |f: &Failures| f.locked(now) || f.times.back().is_some_and(|t| now.duration_since(*t) < window);
        inner.failures.retain(|_, f| keep(f));
        inner.fresh.retain(|_, f| keep(f));
        inner.buckets.retain(|_, b| now.duration_since(b.at) < Duration::from_secs(600));
        inner.known.retain(|_, at| now.duration_since(*at) < KNOWN_FOR);
    }

    #[cfg(test)]
    fn sizes(&self) -> (usize, usize, usize) {
        let inner = self.lock();
        (inner.failures.len(), inner.buckets.len(), inner.known.len())
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
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(guard.reserve(ip, now), Ok(Duration::ZERO));
        }
        assert_eq!(guard.reserve(ip, now), Err(Refused::Locked));
        assert!(guard.reserve(other, now).is_ok());
    }

    #[test]
    fn ipv6_counts_by_its_64() {
        let guard = Guard::new(LoginConfig { max_failures: 3, ..LoginConfig::default() });
        let now = Instant::now();
        for i in 1..=3 {
            let ip: IpAddr = format!("2001:db8:1:2::{i}").parse().unwrap();
            assert!(guard.reserve(ip, now).is_ok());
        }
        let same_64: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        assert_eq!(guard.reserve(same_64, now), Err(Refused::Locked));
        let next_64: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert!(guard.reserve(next_64, now).is_ok());
        // A mapped IPv4 address is that IPv4 address.
        assert_eq!(address_key("::ffff:192.0.2.7".parse().unwrap()), "192.0.2.7".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn many_failures_slow_strangers_but_not_the_owner() {
        let config = LoginConfig { max_failures: 100, global_max_failures: 5, slow_per_minute: 6, ..LoginConfig::default() };
        let guard = Guard::new(config);
        let owner: IpAddr = "198.51.100.7".parse().unwrap();
        guard.succeeded(owner);
        let now = Instant::now();
        // A distributed attack: every attempt from another address.
        for i in 0..5 {
            assert_eq!(guard.reserve(format!("203.0.113.{i}").parse().unwrap(), now), Ok(Duration::ZERO));
        }
        // Slowed down: a stranger takes the one token, the next waits its
        // turn (10 s at 6 a minute), and the one after is too far off.
        assert_eq!(guard.reserve("203.0.113.50".parse().unwrap(), now), Ok(Duration::ZERO));
        let wait = guard.reserve("203.0.113.51".parse().unwrap(), now).unwrap();
        assert!(wait > Duration::from_secs(9) && wait <= MAX_WAIT, "{wait:?}");
        assert_eq!(guard.reserve("203.0.113.52".parse().unwrap(), now), Err(Refused::Busy));
        // The owner's address is not held back.
        assert_eq!(guard.reserve(owner, now), Ok(Duration::ZERO));
    }

    #[test]
    fn fresh_codes_are_counted_per_device() {
        let guard = Guard::new(LoginConfig { max_failures: 2, global_max_failures: 1, ..LoginConfig::default() });
        // Anonymous failures everywhere do not touch a device's codes.
        for i in 0..20 {
            let _ = guard.reserve(format!("203.0.113.{i}").parse().unwrap(), Instant::now());
        }
        assert!(guard.begin_fresh("phone").is_ok());
        guard.fresh_ok("phone");
        assert!(guard.begin_fresh("phone").is_ok());
        assert!(guard.begin_fresh("phone").is_ok());
        assert_eq!(guard.begin_fresh("phone"), Err(Refused::Locked));
        assert!(guard.begin_fresh("laptop").is_ok());
    }

    #[test]
    fn the_maps_stay_bounded() {
        let guard = Guard::new(LoginConfig { global_max_failures: 1_000_000, ..LoginConfig::default() });
        let now = Instant::now();
        for i in 0..(MAX_ENTRIES as u32 + 500) {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            let _ = guard.reserve(ip, now);
            guard.take("x", ip, 5);
        }
        let (failures, buckets, _) = guard.sizes();
        assert!(failures <= MAX_ENTRIES && buckets <= MAX_ENTRIES, "{failures} {buckets}");
    }

    #[test]
    fn buckets_run_out() {
        let guard = Guard::new(LoginConfig::default());
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(guard.take("x", ip, 2));
        assert!(guard.take("x", ip, 2));
        assert!(!guard.take("x", ip, 2));
        assert!(guard.take("y", ip, 2));
        assert!(guard.take_for("n", "w1", 30, 2) && guard.take_for("n", "w1", 30, 2));
        assert!(!guard.take_for("n", "w1", 30, 2));
    }
}
