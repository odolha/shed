//! Desktop notifications, through the desktop's own tool: `notify-send` on
//! Linux, `osascript` on macOS. Nothing happens where neither is found.

use std::io::Read;
use std::process::{Command, Stdio};

/// A notification that says when it is clicked: `notify-send` waits with a
/// default action and prints its name then. The channel gets `()` on a
/// click; it closes when the notification goes otherwise. (macOS's
/// `osascript` cannot tell: there it is a plain notification.)
pub fn send_clickable(title: &str, body: &str) -> async_channel::Receiver<()> {
    let (tx, rx) = async_channel::bounded(1);
    if cfg!(target_os = "macos") {
        send(title, body);
        return rx;
    }
    let mut cmd = Command::new("notify-send");
    cmd.args(["--app-name=Shed", "--icon=dev.shed", "--action=default=Open", title, body]);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return rx };
    // It waits until the notification is clicked, closed or expires: on a
    // thread of its own, not the UI's or the executor's.
    std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_string(&mut out);
        }
        let _ = child.wait();
        if out.trim() == "default" {
            let _ = tx.send_blocking(());
        }
    });
    rx
}

pub fn send(title: &str, body: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut c = Command::new("osascript");
        c.arg("-e").arg(format!("display notification \"{}\" with title \"{}\"", quote(body), quote(title)));
        c
    } else {
        let mut c = Command::new("notify-send");
        c.args(["--app-name=Shed", "--icon=dev.shed", title, body]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        // Reaped away from the UI thread.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
