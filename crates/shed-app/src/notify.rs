//! Desktop notifications, through the desktop's own tool: `notify-send` on
//! Linux, `osascript` on macOS. Nothing happens where neither is found.

use std::process::{Command, Stdio};

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
