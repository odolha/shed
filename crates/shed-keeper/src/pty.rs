//! Spawning shells on pseudo-terminals.

use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::process::Stdio;

use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::pty::{Winsize, openpty};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

pub struct SpawnOptions {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Added to the keeper's own environment.
    pub env: Vec<(OsString, OsString)>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Pty {
    /// The master side; non-blocking and close-on-exec.
    pub master: OwnedFd,
    pub child: Child,
}

fn winsize(cols: u16, rows: u16) -> Winsize {
    Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 }
}

/// Starts `opts.program` as a session leader with a new PTY as its controlling terminal.
pub fn spawn(opts: SpawnOptions) -> io::Result<Pty> {
    let pair = openpty(Some(&winsize(opts.cols, opts.rows)), None).map_err(io::Error::from)?;
    let master = pair.master;
    let slave = pair.slave;

    fcntl(&master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
    let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL).map_err(io::Error::from)?);
    fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(io::Error::from)?;

    let mut cmd = Command::new(&opts.program);
    cmd.args(&opts.args)
        .current_dir(&opts.cwd)
        .envs(opts.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave))
        .kill_on_drop(false);

    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd.spawn()?;
    // `cmd` holds the parent's copies of the slave; drop them so the master
    // sees EOF once the child and its descendants close theirs.
    drop(cmd);
    Ok(Pty { master, child })
}

pub fn resize(master: BorrowedFd<'_>, cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> io::Result<()> {
    let ws = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: cols.saturating_mul(cell_w),
        ws_ypixel: rows.saturating_mul(cell_h),
    };
    // SAFETY: TIOCSWINSZ reads a Winsize from a valid pointer.
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// The process group in the foreground of the PTY: the pane's current tool, or the shell.
pub fn foreground_pgrp(master: &impl AsFd) -> Option<Pid> {
    // A terminal whose shell has gone reports 0, and signalling group 0 would
    // signal the keeper's own group.
    nix::unistd::tcgetpgrp(master.as_fd()).ok().filter(|p| p.as_raw() > 0)
}

/// The user's login shell: `$SHELL`, then the passwd entry, then `/bin/sh`.
pub fn default_shell() -> PathBuf {
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return shell.into();
    }
    nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|u| u.shell)
        .unwrap_or_else(|| "/bin/sh".into())
}
