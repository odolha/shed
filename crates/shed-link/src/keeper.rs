//! Asking the keeper for things on behalf of a call.
//!
//! The link's own connection hears the tree, outputs and AI updates, but
//! the keeper's `Done` and `Error` answers are not tied to the message they
//! answer. A change whose answer matters goes over a connection of its own
//! (a CLI client), so its answer is its own; what the change made (a tab, a
//! pane) is found in the tree the link's connection hears.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use shed_api::{ApiError, ErrorCode};
use shed_model::Shed;
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg};
use tokio::sync::watch;

/// How long a change may take to show.
pub(crate) const WAIT: Duration = Duration::from_secs(4);

/// What the keeper said to a change sent on its own connection.
#[derive(Debug)]
pub(crate) enum Answer {
    Done,
    /// Nothing said in time (many changes are answered by the tree only).
    Quiet,
}

/// The keeper's way of saying a bench was opened but no window shows it:
/// not a failure for a link, which needs no window.
fn no_window(e: &str) -> bool {
    e.contains("no window is open")
}

/// Sends `msg` on a connection of its own and waits up to `wait` for the
/// keeper's `Done` or `Error`.
pub(crate) async fn ask(socket: &Path, msg: ClientMsg, wait: Duration) -> Result<Answer, ApiError> {
    let socket = socket.to_path_buf();
    let (conn, rx) = tokio::task::spawn_blocking(move || shed_client::keeper_conn::connect_at(&socket, ClientKind::Cli))
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(|e| ApiError::new(ErrorCode::Failed, format!("{e:#}")))?;
    conn.send_msg(msg);
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => return Ok(Answer::Quiet),
            Ok(Err(_)) => return Err(failed("the keeper closed the connection")),
            Ok(Ok(Frame::Keeper(KeeperMsg::Done(_)))) => return Ok(Answer::Done),
            Ok(Ok(Frame::Keeper(KeeperMsg::Error(e)))) if no_window(&e) => return Ok(Answer::Quiet),
            Ok(Ok(Frame::Keeper(KeeperMsg::Error(e)))) => return Err(failed(e)),
            Ok(Ok(_)) => {}
        }
    }
}

/// Waits until `find` finds something in the tree (as it is now, or as it
/// becomes) for up to `wait`.
pub(crate) async fn find_in_tree<T>(tree: &mut watch::Receiver<Arc<Shed>>, wait: Duration, mut find: impl FnMut(&Shed) -> Option<T>) -> Option<T> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if let Some(found) = find(&tree.borrow_and_update()) {
            return Some(found);
        }
        match tokio::time::timeout_at(deadline, tree.changed()).await {
            Ok(Ok(())) => {}
            _ => return None,
        }
    }
}

/// Sends `msg` on its own connection and waits for what it made to show in
/// the tree. A keeper error fails it; a quiet keeper does not.
pub(crate) async fn make<T>(socket: &Path, tree: &watch::Receiver<Arc<Shed>>, msg: ClientMsg, find: impl FnMut(&Shed) -> Option<T>) -> Result<T, ApiError> {
    let mut tree = tree.clone();
    let asked = ask(socket, msg, WAIT);
    let found = find_in_tree(&mut tree, WAIT, find);
    tokio::pin!(asked, found);
    let mut answered = false;
    loop {
        tokio::select! {
            r = &mut asked, if !answered => {
                r?;
                answered = true;
            }
            f = &mut found => return f.ok_or_else(|| ApiError::new(ErrorCode::Timeout, "the keeper did not make it in time")),
        }
    }
}

pub(crate) fn failed(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Failed, msg)
}
