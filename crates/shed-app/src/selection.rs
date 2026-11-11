//! Which of the X11 selections changed last, so middle-click pastes what was
//! copied or selected most recently, wherever that happened.
//!
//! By convention middle-click pastes the primary selection: the text selected
//! last. Text copied without selecting it (a page's copy button, a file
//! manager, an IDE action) reaches only the clipboard, and the primary
//! selection still holds something older. A thread hears from XFixes whenever
//! either selection gets an owner and notes the order. It sleeps until then:
//! an idle Shed is not woken.

use std::sync::atomic::{AtomicU64, Ordering};

use tracing::debug;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Primary,
    Clipboard,
}

/// The place of each selection's last change among all changes seen; 0 for none yet.
static PRIMARY: AtomicU64 = AtomicU64::new(0);
static CLIPBOARD: AtomicU64 = AtomicU64::new(0);

/// The selection that changed last since Shed started, if either did.
pub fn newest() -> Option<Kind> {
    newer(PRIMARY.load(Ordering::Relaxed), CLIPBOARD.load(Ordering::Relaxed))
}

fn newer(primary: u64, clipboard: u64) -> Option<Kind> {
    match primary.cmp(&clipboard) {
        std::cmp::Ordering::Greater => Some(Kind::Primary),
        std::cmp::Ordering::Less => Some(Kind::Clipboard),
        std::cmp::Ordering::Equal => None,
    }
}

/// Starts listening. On Wayland nothing tells which changed last, and
/// middle-click keeps to the primary selection.
pub fn watch() {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty()) {
        return;
    }
    let spawned = std::thread::Builder::new().name("selections".into()).spawn(|| {
        if let Err(e) = listen() {
            debug!("not following the X11 selections: {e}");
        }
    });
    if let Err(e) = spawned {
        debug!("cannot start the selection thread: {e}");
    }
}

fn listen() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let primary: u32 = AtomEnum::PRIMARY.into();
    let clipboard = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
    for selection in [primary, clipboard] {
        conn.xfixes_select_selection_input(root, selection, SelectionEventMask::SET_SELECTION_OWNER)?;
    }
    conn.flush()?;
    let mut changes = 0;
    loop {
        if let Event::XfixesSelectionNotify(e) = conn.wait_for_event()? {
            changes += 1;
            let last = if e.selection == primary { &PRIMARY } else { &CLIPBOARD };
            last.store(changes, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_later_change_wins() {
        assert_eq!(newer(0, 0), None);
        assert_eq!(newer(0, 1), Some(Kind::Clipboard));
        assert_eq!(newer(3, 2), Some(Kind::Primary));
    }
}
