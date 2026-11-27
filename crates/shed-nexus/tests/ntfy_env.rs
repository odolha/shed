//! The ntfy topic and token from the environment (`/etc/shed-nexus.env`),
//! the topic winning over `nexus.toml`'s. One test alone in its binary, as it
//! sets environment variables.

mod common;
mod receiver;

use common::*;
use receiver::*;
use shed_api::http::PushInfo;
use shed_api::wire::*;

#[tokio::test]
async fn the_env_names_the_topic_and_its_token() {
    let receiver = Receiver::start().await;
    let topic = format!("{}/alerts-env", receiver.base);
    // SAFETY: the only test in this binary, before anything reads the
    // environment on other threads.
    unsafe {
        std::env::set_var("SHED_NEXUS_NTFY_URL", &topic);
        std::env::set_var("SHED_NEXUS_NTFY_TOKEN", "tk_from_env");
    }
    let nexus = start_with(|c| c.push.ntfy_url = Some(format!("{}/alerts-toml", receiver.base))).await;
    let mut account = set_up(&nexus).await;
    let (_, token) = pair(&nexus, &mut account, "desk").await;
    // The pairing was a security alert, to the topic.
    let alerts = receiver.wait_for(1).await;
    assert_eq!(alerts[0].headers["click"], "shed://devices");
    receiver.clear();
    let mut link = link(&nexus, &token).await;
    let info: PushInfo = nexus.get("/api/v1/push", &account.browser).await;
    assert_eq!(info.ntfy_topic.as_deref(), Some(topic.as_str()), "the topic, without its token");

    let notification = Notification {
        kind: NotificationKind::CommandDone,
        title: "cargo build finished".into(),
        body: "api · bash".into(),
        bench: None,
        tab: None,
        present: false,
    };
    send(&mut link, &LinkFrame::Notify(notification)).await;
    let hits = receiver.wait_for(1).await;
    let [hit] = &hits[..] else { panic!("one message: {hits:?}") };
    assert_eq!(hit.path, "/alerts-env");
    assert_eq!(hit.headers["authorization"], "Bearer tk_from_env");
    assert_eq!(hit.headers["priority"], "3");
    assert_eq!(hit.headers["title"], "cargo build finished");
}
