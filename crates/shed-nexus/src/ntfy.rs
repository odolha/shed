//! Notifications to phones through a plain ntfy topic (`[push] ntfy_url`):
//! one POST per notification, the text as the body and the rest in ntfy's
//! headers (`Title`, `Priority`, `Tags`, `Click`), with the publishing
//! account's token as `Authorization: Bearer`. The phone reads the topic in
//! the ntfy app; tapping a notification opens `shed://w/…/b/…/t/…`, which
//! the Android app takes. Unlike Web Push the text is not encrypted, so the
//! topic belongs on a ntfy you run (`auth-default-access: deny-all`).

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use shed_api::wire::{Notification, NotificationKind};

use crate::config::NtfyTopic;

pub struct Ntfy {
    topic: NtfyTopic,
    client: reqwest::Client,
}

/// A ntfy message: what goes in its headers and body.
#[derive(Debug, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    pub priority: &'static str,
    pub tags: &'static str,
    pub click: Option<String>,
}

impl Ntfy {
    pub fn new(topic: NtfyTopic) -> anyhow::Result<Ntfy> {
        crate::install_crypto();
        // The topic is the admin's own setting, not a device's: it may be on
        // this host or the private network, unlike Web Push endpoints.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Ntfy { topic, client })
    }

    /// The topic's URL, as apps are told (no token).
    pub fn url(&self) -> &str {
        &self.topic.url
    }

    pub async fn send(&self, message: &Message) {
        let mut request = self
            .client
            .post(&self.topic.url)
            .header("Title", header_text(&message.title))
            .header("Priority", message.priority)
            .header("Tags", message.tags)
            .body(message.body.clone());
        if let Some(click) = &message.click {
            request = request.header("Click", click.as_str());
        }
        if let Some(token) = &self.topic.token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(answer) if !answer.status().is_success() => tracing::warn!("ntfy: http {}", answer.status()),
            Ok(_) => {}
            Err(e) => tracing::warn!("ntfy: {e}"),
        }
    }
}

/// The ntfy message for a notification from `workspace` (named `name`).
pub fn message(workspace: &str, name: &str, notification: &Notification) -> Message {
    let (priority, tags) = match notification.kind {
        NotificationKind::AiWaiting => ("4", "question"),
        NotificationKind::AiDone => ("3", "white_check_mark"),
        NotificationKind::CommandDone => ("3", "checkered_flag"),
        NotificationKind::WorkspaceOffline => ("2", "warning"),
        NotificationKind::Security => ("4", "lock"),
    };
    let body = match (name.is_empty(), notification.body.is_empty()) {
        (false, false) => format!("{name} · {}", notification.body),
        (false, true) => name.to_string(),
        (true, false) => notification.body.clone(),
        (true, true) => notification.title.clone(),
    };
    Message { title: notification.title.clone(), body, priority, tags, click: Some(link(workspace, notification)) }
}

/// The ntfy message for a security alert from the nexus: priority 5 (max)
/// when `urgent`, else 4; a tap opens the Android app's Devices.
pub fn alert_message(notification: &Notification, urgent: bool) -> Message {
    Message {
        title: notification.title.clone(),
        body: if notification.body.is_empty() { notification.title.clone() } else { notification.body.clone() },
        priority: if urgent { "5" } else { "4" },
        tags: "lock",
        click: Some(DEVICES_LINK.to_string()),
    }
}

/// The Android app's Devices screen, where a device can be revoked.
pub const DEVICES_LINK: &str = "shed://devices";

/// The Android app's deep link to where it happened: `shed://w/<workspace>`,
/// with `/b/<bench>` and `/t/<tab>` as far as they are known.
pub fn link(workspace: &str, notification: &Notification) -> String {
    let mut link = format!("shed://w/{}", segment(workspace));
    if let Some(bench) = &notification.bench {
        link.push_str(&format!("/b/{}", segment(bench)));
        if let Some(tab) = &notification.tab {
            link.push_str(&format!("/t/{}", segment(tab)));
        }
    }
    link
}

/// A path segment, percent-encoded beyond letters, digits and `-_.~`.
fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A header's text: as is when it is printable ASCII, else RFC 2047
/// (`=?UTF-8?B?…?=`), which ntfy decodes.
fn header_text(s: &str) -> String {
    if s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        s.to_string()
    } else {
        format!("=?UTF-8?B?{}?=", STANDARD.encode(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(kind: NotificationKind, bench: Option<&str>, tab: Option<&str>) -> Notification {
        Notification {
            kind,
            title: "Claude waits for an answer".into(),
            body: "api · claude".into(),
            bench: bench.map(Into::into),
            tab: tab.map(Into::into),
            present: false,
        }
    }

    #[test]
    fn messages_say_where_and_how_urgent() {
        let m = message("w1", "desk", &notification(NotificationKind::AiWaiting, Some("b1"), Some("t1")));
        assert_eq!(m.title, "Claude waits for an answer");
        assert_eq!(m.body, "desk · api · claude");
        assert_eq!(m.priority, "4");
        assert_eq!(m.click.as_deref(), Some("shed://w/w1/b/b1/t/t1"));
        let m = message("w1", "desk", &notification(NotificationKind::AiDone, Some("b1"), None));
        assert_eq!((m.priority, m.click.as_deref()), ("3", Some("shed://w/w1/b/b1")));
        let offline = Notification {
            kind: NotificationKind::WorkspaceOffline,
            title: "desk went offline".into(),
            body: String::new(),
            bench: None,
            tab: None,
            present: false,
        };
        let m = message("w 1/x", "desk", &offline);
        assert_eq!((m.priority, m.body.as_str(), m.click.as_deref()), ("2", "desk", Some("shed://w/w%201%2Fx")));
    }

    #[test]
    fn alerts_are_urgent_and_open_the_devices() {
        let alert = Notification {
            kind: NotificationKind::Security,
            title: "New sign-in: Pixel".into(),
            body: "A phone from 203.0.113.5".into(),
            bench: None,
            tab: None,
            present: false,
        };
        let m = alert_message(&alert, true);
        assert_eq!((m.priority, m.tags, m.click.as_deref()), ("5", "lock", Some("shed://devices")));
        assert_eq!(m.body, "A phone from 203.0.113.5");
        assert_eq!(alert_message(&alert, false).priority, "4");
    }

    #[test]
    fn titles_beyond_ascii_are_encoded() {
        assert_eq!(header_text("cargo build finished"), "cargo build finished");
        assert_eq!(header_text("make…"), format!("=?UTF-8?B?{}?=", STANDARD.encode("make…")));
        assert!(header_text("a\nb").starts_with("=?UTF-8?B?"));
    }
}
