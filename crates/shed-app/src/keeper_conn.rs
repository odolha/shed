//! The window's connection to the keeper, starting the keeper if needed.

use std::io::{BufReader, BufWriter, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use shed_model::paths;
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, ProtoError, read_frame};

/// Reported when the keeper speaks another protocol: it is from an older
/// or newer build of Shed.
pub const OTHER_VERSION: &str = "the running keeper is from another version of Shed";

/// Whether a keeper error means it is from another version of Shed.
pub fn is_other_version(error: &str) -> bool {
    error.starts_with(OTHER_VERSION) || error.starts_with("protocol mismatch")
}

/// Sends frames to the keeper. Cheap to clone; a writer thread does the I/O.
#[derive(Clone)]
pub struct KeeperConn {
    tx: mpsc::Sender<Frame>,
    /// The keeper's process, read from the socket, so known whatever its version.
    pub keeper_pid: Option<u32>,
}

impl KeeperConn {
    pub fn send(&self, frame: Frame) {
        let _ = self.tx.send(frame);
    }

    pub fn send_msg(&self, msg: ClientMsg) {
        self.send(Frame::Client(msg));
    }
}

/// Connects to the keeper, starting `shedd` if its socket is missing or dead.
/// Frames from the keeper arrive on the returned channel, which closes when
/// the keeper goes away.
pub fn connect(kind: ClientKind, start_keeper: bool) -> anyhow::Result<(KeeperConn, async_channel::Receiver<Frame>)> {
    let socket = paths::socket_path();
    let stream = match UnixStream::connect(&socket) {
        Ok(s) => s,
        Err(_) if start_keeper => {
            spawn_keeper()?;
            wait_for_socket(&socket)?
        }
        Err(e) => return Err(e).with_context(|| format!("the keeper is not running ({})", socket.display())),
    };

    let (out_tx, out_rx) = mpsc::channel::<Frame>();
    let mut writer = BufWriter::new(stream.try_clone()?);
    std::thread::Builder::new().name("keeper-write".into()).spawn(move || {
        let mut buf = Vec::new();
        while let Ok(frame) = out_rx.recv() {
            buf.clear();
            frame.encode_into(&mut buf);
            // Coalesce whatever else is queued into one write.
            while let Ok(more) = out_rx.try_recv() {
                more.encode_into(&mut buf);
            }
            if writer.write_all(&buf).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    })?;

    let keeper_pid = peer_pid(&stream);
    let (in_tx, in_rx) = async_channel::unbounded();
    let mut reader = BufReader::with_capacity(256 * 1024, stream);
    std::thread::Builder::new().name("keeper-read".into()).spawn(move || {
        loop {
            let frame = match read_frame(&mut reader) {
                // Messages travel by position, so another version's may not
                // decode at all, or its error may read as something else; its
                // welcome always says which version it is.
                Ok(Frame::Keeper(KeeperMsg::Welcome { version, .. })) if version != PROTOCOL_VERSION => {
                    Frame::Keeper(KeeperMsg::Error(format!("{OTHER_VERSION} (protocol {version}, this one {PROTOCOL_VERSION})")))
                }
                Ok(frame) => frame,
                Err(ProtoError::Io(_)) => break,
                Err(_) => {
                    let _ = in_tx.send_blocking(Frame::Keeper(KeeperMsg::Error(OTHER_VERSION.into())));
                    break;
                }
            };
            if in_tx.send_blocking(frame).is_err() {
                break;
            }
        }
    })?;

    let conn = KeeperConn { tx: out_tx, keeper_pid };
    conn.send_msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind });
    Ok((conn, in_rx))
}

/// The process at the other end of a Unix socket (`SO_PEERCRED`).
#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` describe a writable buffer of the size the option fills.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (rc == 0 && cred.pid > 0).then_some(cred.pid as u32)
}

/// The process at the other end of a Unix socket (`LOCAL_PEERPID`).
#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` describe a writable buffer of the size the option fills.
    let rc = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len)
    };
    (rc == 0 && pid > 0).then_some(pid as u32)
}

/// Whether a process is still running (a zombie counts as gone).
#[cfg(target_os = "linux")]
pub fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "))
}

/// Whether a process is still running.
#[cfg(not(target_os = "linux"))]
pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn keeper_binary() -> PathBuf {
    // Through any symlink (`shed` linked onto PATH from Shed.app) to the real one.
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| exe.parent().map(|dir| dir.join("shedd")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("shedd"))
}

/// Starts `shedd` in its own session, so it outlives this process.
fn spawn_keeper() -> anyhow::Result<()> {
    let bin = keeper_binary();
    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().with_context(|| format!("starting the keeper ({})", bin.display()))?;
    // The keeper stays in the background; reap it here if it exits early so it
    // does not linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn wait_for_socket(socket: &std::path::Path) -> anyhow::Result<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(socket) {
            Ok(s) => return Ok(s),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => bail!("the keeper did not start: {e} (see {})", paths::keeper_log_path().display()),
        }
    }
}
