//! Logout and reboot: the system stops the keeper and every program at once.
//! The keeper must still save each bench with the tools that were running.

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, ShowTarget, read_frame, write_frame};

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_saves_benches_even_as_their_tools_die() {
    let dir = tempfile::tempdir().unwrap();
    let d = |p: &str| dir.path().join(p);
    for sub in ["home", "run", "data", "state", "config"] {
        std::fs::create_dir_all(d(sub)).unwrap();
    }
    let mut keeper = Command::new(env!("CARGO_BIN_EXE_shedd"))
        .arg("--foreground")
        .env("HOME", d("home"))
        .env("SHELL", "/bin/bash")
        .env("SHED_RUNTIME_DIR", d("run"))
        .env("XDG_DATA_HOME", d("data"))
        .env("XDG_STATE_HOME", d("state"))
        .env("XDG_CONFIG_HOME", d("config"))
        .env("PS1", "$ ")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // The first connection that succeeds is the one used: a keeper holding
    // nothing exits as soon as its last client leaves.
    let socket = d("run/shedd.sock");
    let mut conn = None;
    wait_for("the socket", || {
        conn = UnixStream::connect(&socket).ok();
        conn.is_some()
    });
    let mut stream = conn.unwrap();
    let mut read = BufReader::new(stream.try_clone().unwrap());
    let (tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(f) = read_frame(&mut read) {
            let _ = tx.send(f);
        }
    });
    let mut send = |f: Frame| {
        write_frame(&mut stream, &f).unwrap();
        stream.flush().unwrap();
    };
    send(Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window }));
    send(Frame::Client(ClientMsg::Show(ShowTarget::NewBench { name: Some("night".into()), cwd: None })));
    let mut tree = None;
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let pane = loop {
        match frames.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("a tree") {
            Frame::Keeper(KeeperMsg::Show { bench }) => send(Frame::Client(ClientMsg::Attach { bench: Some(bench) })),
            Frame::Keeper(KeeperMsg::TreeChanged(t)) => tree = Some(t),
            Frame::Output { pane, bytes } => {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains("133;B") {
                    break pane;
                }
            }
            _ => {}
        }
    };
    assert!(tree.is_some());
    send(Frame::Input { pane, bytes: b"sleep 300\n".to_vec() });

    // The sleep's pid, from its process group: wait until it runs.
    let sleep_pid = || -> Option<i32> {
        let out = Command::new("pgrep").args(["-x", "sleep", "-P"]).arg(
            Command::new("pgrep").args(["-x", "bash", "-P", &keeper.id().to_string()]).output().ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?,
        ).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    };
    wait_for("sleep to start", || sleep_pid().is_some());
    std::thread::sleep(Duration::from_millis(300));

    // Logout: the tool and the keeper are told to stop together.
    let _ = kill(Pid::from_raw(sleep_pid().unwrap()), Signal::SIGTERM);
    let _ = kill(Pid::from_raw(keeper.id() as i32), Signal::SIGTERM);
    let status = keeper.wait().unwrap();
    assert!(status.success() || status.code().is_some());

    let saved = std::fs::read_to_string(d("data/shed/benches/night.toml")).expect("the bench was saved");
    assert!(saved.contains("tool = \"sleep 300\""), "the tool that died at logout is saved:\n{saved}");
    let session = std::fs::read_to_string(d("data/shed/session.toml")).unwrap();
    assert!(session.contains("night"), "{session}");
}
