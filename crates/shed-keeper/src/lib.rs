//! The keeper (`shedd`): owns every PTY so that closing a window kills nothing.

pub mod pty;
mod server;

use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

pub use server::{Keeper, KeeperConfig};
use tokio::net::UnixListener;

/// Creates the runtime directory (0700) if needed.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// Takes the keeper lock. `Ok(None)` means another keeper already holds it.
pub fn take_lock(path: &Path) -> io::Result<Option<File>> {
    let file = File::options().create(true).truncate(false).write(true).open(path)?;
    // SAFETY: flock on a valid, owned descriptor.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(Some(file))
    } else {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(err) }
    }
}

/// Binds the keeper socket with permission 0600, replacing a stale socket file.
/// Call only while holding the lock.
pub fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
