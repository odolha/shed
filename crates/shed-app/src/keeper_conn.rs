//! The window's connection to the keeper, starting the keeper if needed.

use std::io::{BufReader, BufWriter, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use shed_model::paths;
use shed_proto::{ClientKind, ClientMsg, Frame, PROTOCOL_VERSION, read_frame};

/// Sends frames to the keeper. Cheap to clone; a writer thread does the I/O.
#[derive(Clone)]
pub struct KeeperConn {
    tx: mpsc::Sender<Frame>,
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

    let (in_tx, in_rx) = async_channel::unbounded();
    let mut reader = BufReader::with_capacity(256 * 1024, stream);
    std::thread::Builder::new().name("keeper-read".into()).spawn(move || {
        while let Ok(frame) = read_frame(&mut reader) {
            if in_tx.send_blocking(frame).is_err() {
                break;
            }
        }
    })?;

    let conn = KeeperConn { tx: out_tx };
    conn.send_msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind });
    Ok((conn, in_rx))
}

fn keeper_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
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
