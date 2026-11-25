//! How long since anyone used the computer, as its desktop tells: X11's
//! screen saver extension, GNOME's or KDE's idle time on Wayland, the HID
//! idle time on macOS. `None` without a desktop (a server, over ssh) or when
//! it does not say; the keeper then goes by Shed's own clients alone.

use std::process::Command;
use std::time::Duration;

/// Time since the last key, click or mouse move anywhere on this computer.
/// Blocking (it asks the display server or a program): call it off the
/// async runtime.
#[cfg(target_os = "linux")]
pub fn desktop_idle() -> Option<Duration> {
    let set = |var| std::env::var_os(var).is_some_and(|v| !v.is_empty());
    // A Wayland session has an X server too (Xwayland), which sees only its
    // own clients' input: ask the compositor instead.
    if set("WAYLAND_DISPLAY") {
        wayland()
    } else if set("DISPLAY") {
        x11()
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn x11() -> Option<Duration> {
    use x11rb::connection::Connection;
    use x11rb::protocol::screensaver::ConnectionExt;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let info = conn.screensaver_query_info(root).ok()?.reply().ok()?;
    Some(Duration::from_millis(info.ms_since_user_input.into()))
}

/// GNOME (Mutter's idle monitor), else KDE and others that offer
/// `org.freedesktop.ScreenSaver.GetSessionIdleTime`; both in milliseconds.
#[cfg(target_os = "linux")]
fn wayland() -> Option<Duration> {
    let ask = |dest: &str, path: &str, method: &str| {
        let out = Command::new("gdbus")
            .args(["call", "--session", "--timeout", "1", "--dest", dest, "--object-path", path, "--method", method])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        first_number(&String::from_utf8_lossy(&out.stdout)).map(Duration::from_millis)
    };
    ask("org.gnome.Mutter.IdleMonitor", "/org/gnome/Mutter/IdleMonitor/Core", "org.gnome.Mutter.IdleMonitor.GetIdletime").or_else(|| {
        ask("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver.GetSessionIdleTime")
    })
}

/// `HIDIdleTime` (nanoseconds) from `ioreg`.
#[cfg(target_os = "macos")]
pub fn desktop_idle() -> Option<Duration> {
    let out = Command::new("ioreg").args(["-c", "IOHIDSystem", "-d", "4"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
    first_number(line.split('=').nth(1)?).map(Duration::from_nanos)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn desktop_idle() -> Option<Duration> {
    None
}

/// The first whole number in `text`: gdbus says `(uint64 1234,)`, ioreg
/// ` 1234`.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
fn first_number(text: &str) -> Option<u64> {
    let text = text.strip_prefix('(').unwrap_or(text);
    // Skip a type word ("uint64") before the value.
    let value = text.split_whitespace().find(|w| w.starts_with(|c: char| c.is_ascii_digit()))?;
    value.trim_end_matches([',', ')']).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::first_number;

    #[test]
    fn reads_what_gdbus_and_ioreg_say() {
        assert_eq!(first_number("(uint64 12345,)\n"), Some(12345));
        assert_eq!(first_number("(uint32 7,)"), Some(7));
        assert_eq!(first_number(" 987654321"), Some(987654321));
        assert_eq!(first_number("(true,)"), None);
    }
}
