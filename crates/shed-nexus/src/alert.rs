//! Security alerts: the nexus tells the account's devices about a sign-in, a
//! workspace paired, the password changed, a device revoked or an address
//! locked out (`NotificationKind::Security`).
//!
//! Unlike a workspace's notifications they go to every device but the one
//! that caused them, live to every connection and pushed as well, whether or
//! not an app is in view and whatever kinds a device chose; nothing marks
//! them `present`. Their recipients are found at once, before the change
//! (a password change revokes the other devices; their alert still goes).
//! A ntfy topic is shared by all phones, so a phone that caused one hears it
//! there too. Lockout alerts are throttled (`Throttle`), so an attack cannot
//! flood the phone.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shed_api::http::DeviceKind;
use shed_api::wire::{Notification, NotificationFrame, NotificationKind};

use crate::App;
use crate::http::Client;

/// A security alert.
#[derive(Clone, Debug)]
pub struct Alert {
    pub title: String,
    pub body: String,
    /// The most urgent level (ntfy priority 5): a sign-in from a new
    /// address, the password changed, an address locked out. Else 4.
    pub urgent: bool,
    /// The device that caused it, which is not told.
    pub except: Option<String>,
}

/// Sends `alert`: live to every connection (but the causing device's), and
/// pushed to the ntfy topic and every Web Push subscription (but the
/// causing device's; phones read the topic when there is one).
pub fn send(app: &Arc<App>, alert: Alert) {
    tracing::info!("security alert: {}: {}", alert.title, alert.body);
    let notification = Notification {
        kind: NotificationKind::Security,
        title: crate::push::clip(&alert.title, crate::push::TITLE_MAX),
        body: crate::push::clip(&alert.body, crate::push::BODY_MAX),
        bench: None,
        tab: None,
        present: false,
    };
    let frame = NotificationFrame { workspace: String::new(), notification: notification.clone() };
    app.hub.notify_all_but(frame, alert.except.as_deref());
    // Who to push to is decided now: after a password change, the devices
    // it signed out are no longer subscribed.
    let subs = match (&app.push, app.store.subscriptions(NotificationKind::Security)) {
        (Some(_), Ok(subs)) => subs
            .into_iter()
            .filter(|s| Some(s.device.as_str()) != alert.except.as_deref())
            .filter(|s| !(app.ntfy.is_some() && s.kind == DeviceKind::Phone))
            .collect(),
        (Some(_), Err(e)) => {
            tracing::warn!("push: {e:#}");
            Vec::new()
        }
        (None, _) => Vec::new(),
    };
    if subs.is_empty() && app.ntfy.is_none() {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        if let Some(ntfy) = &app.ntfy {
            ntfy.send(&crate::ntfy::alert_message(&notification, alert.urgent)).await;
        }
        crate::push::deliver_all(&app, subs, &notification, "", String::new()).await;
    });
}

/// A device kind as alerts name it.
pub fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Browser => "a browser",
        DeviceKind::Phone => "a phone",
        DeviceKind::Desktop => "Shed's desktop app",
        DeviceKind::Workspace => "a workspace",
    }
}

/// `203.0.113.5 (DE)`, or the address alone.
pub fn place(address: &str, country: Option<&str>) -> String {
    match country {
        Some(country) => format!("{address} ({country})"),
        None => address.to_string(),
    }
}

/// The alert for a sign-in of `kind` named `name` from `client`; `familiar`
/// when the address signed in or was seen before.
pub fn signed_in(client: &Client, kind: DeviceKind, name: &str, device: &str, familiar: bool) -> Alert {
    let mut body = format!("{} from {}", capital(kind_name(kind)), place(&client.address(), client.country.as_deref()));
    if !familiar {
        body.push_str(", an address not seen before");
    }
    body.push_str(". Not you? Revoke it in Devices and change the password.");
    Alert { title: format!("New sign-in: {name}"), body, urgent: !familiar, except: Some(device.to_string()) }
}

fn capital(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// How often lockout alerts go: at most one per address an hour, and
/// `GLOBAL_PER_HOUR` in all.
pub struct Throttle {
    inner: Mutex<ThrottleInner>,
}

#[derive(Default)]
struct ThrottleInner {
    last: HashMap<IpAddr, Instant>,
    sent: VecDeque<Instant>,
}

const PER_ADDRESS: Duration = Duration::from_secs(3600);
/// Lockout alerts an hour, for all addresses together.
pub const GLOBAL_PER_HOUR: usize = 5;
/// Addresses remembered at most (the oldest go).
const MAX_ADDRESSES: usize = 1000;

impl Default for Throttle {
    fn default() -> Self {
        Throttle { inner: Mutex::new(ThrottleInner::default()) }
    }
}

impl Throttle {
    /// Whether a lockout alert for `ip` (by `auth::address_key`) may go at
    /// `now`; counts it if so.
    pub fn allow(&self, ip: IpAddr, now: Instant) -> bool {
        let key = crate::auth::address_key(ip);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *inner;
        while inner.sent.front().is_some_and(|t| now.duration_since(*t) >= PER_ADDRESS) {
            inner.sent.pop_front();
        }
        inner.last.retain(|_, at| now.duration_since(*at) < PER_ADDRESS);
        if inner.last.contains_key(&key) || inner.sent.len() >= GLOBAL_PER_HOUR {
            return false;
        }
        if inner.last.len() >= MAX_ADDRESSES
            && let Some(oldest) = inner.last.iter().min_by_key(|(_, at)| **at).map(|(k, _)| *k)
        {
            inner.last.remove(&oldest);
        }
        inner.last.insert(key, now);
        inner.sent.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_alerts_are_throttled() {
        let throttle = Throttle::default();
        let start = Instant::now();
        let ip = |i: u8| IpAddr::from([203, 0, 113, i]);
        assert!(throttle.allow(ip(1), start));
        // Once an hour per address (an IPv6 /64 counts as one).
        assert!(!throttle.allow(ip(1), start + Duration::from_secs(600)));
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(throttle.allow(v6("2001:db8::1"), start));
        assert!(!throttle.allow(v6("2001:db8::2"), start));
        // At most a few in all, whatever the address.
        for i in 2..=(GLOBAL_PER_HOUR as u8 - 1) {
            assert!(throttle.allow(ip(i), start), "{i}");
        }
        assert!(!throttle.allow(ip(100), start));
        // An hour on, again.
        let later = start + PER_ADDRESS;
        assert!(throttle.allow(ip(1), later));
        assert!(throttle.allow(ip(100), later));
    }

    #[test]
    fn places_and_names() {
        assert_eq!(place("203.0.113.5", Some("DE")), "203.0.113.5 (DE)");
        assert_eq!(place("203.0.113.5", None), "203.0.113.5");
        assert_eq!(capital(kind_name(DeviceKind::Phone)), "A phone");
    }
}
