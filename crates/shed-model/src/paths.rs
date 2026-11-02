//! XDG locations for config, data, state and the keeper socket.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(fallback),
    }
}

/// `~/.config/shed`
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("shed")
}

/// `~/.local/share`: where desktop entries and icons go.
pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// `~/.local/share/shed`
pub fn data_dir() -> PathBuf {
    data_home().join("shed")
}

/// `~/.local/state/shed`
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("shed")
}

/// `$XDG_RUNTIME_DIR/shed`, falling back to `$TMPDIR/shed` on macOS (a
/// folder of the user's own) and `/tmp/shed-<uid>` elsewhere.
///
/// `SHED_RUNTIME_DIR` overrides it, so tests and a second development keeper
/// can run beside the real one.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SHED_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("shed"),
        _ if cfg!(target_os = "macos") && std::env::var_os("TMPDIR").is_some_and(|d| !d.is_empty()) => {
            PathBuf::from(std::env::var_os("TMPDIR").unwrap()).join("shed")
        }
        _ => {
            // SAFETY: getuid has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!("/tmp/shed-{uid}"))
        }
    }
}

/// The keeper's socket.
pub fn socket_path() -> PathBuf {
    runtime_dir().join("shedd.sock")
}

/// Held with `flock` by the running keeper, so only one starts.
pub fn lock_path() -> PathBuf {
    runtime_dir().join("shedd.lock")
}

pub fn keeper_log_path() -> PathBuf {
    state_dir().join("shedd.log")
}
