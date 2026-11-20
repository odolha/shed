//! XDG locations for config, data, state and the keeper socket.

use std::path::{Path, PathBuf};

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

/// The pairing with a nexus: its workspace id, name and token (0600, and
/// not a `.toml`, so backups leave it out).
pub fn nexus_secret_path() -> PathBuf {
    data_dir().join("nexus.json")
}

/// What the running link says about itself, for `shed nexus status`.
pub fn link_status_path() -> PathBuf {
    runtime_dir().join("link.json")
}

/// The link's own log (the keeper starts it with its output here).
pub fn link_log_path() -> PathBuf {
    state_dir().join("link.log")
}

/// Writes a file whole: to a file beside it, then over it in one step, so a
/// crash or a full disk never leaves it half written.
/// A file that is a link (dotfiles) is written where it points, the link kept.
pub fn write_whole(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = real.as_path();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_written_whole_through_links() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-shed.toml");
        std::fs::write(&real, "old").unwrap();
        let link = dir.path().join("shed.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_whole(&link, "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        write_whole(&dir.path().join("fresh.toml"), "x").unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3, "no temporary file left");
    }
}
