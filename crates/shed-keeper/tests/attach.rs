//! `shedd attach`, the server end of a remote tab, against a keeper: what
//! the shell prints comes out, what is written goes in; letting go keeps the
//! shell, joining again shows it again, and the close request ends it.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shed_keeper::{Keeper, KeeperConfig};

fn start_keeper(dir: &Path) {
    let socket = dir.join("shedd.sock");
    let home = dir.to_path_buf();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&socket).unwrap();
            let keeper = Keeper::new(KeeperConfig {
                socket,
                config_file: home.join("shed.toml"),
                logbook: None,
                state_dir: home.join("state"),
                shed_bin: None,
                env: Vec::new(),
                store: None,
                desktop: None,
                aliases_file: home.join("aliases.toml"),
                link_secret: None,
                link_log: home.join("link.log"),
                shell: Some("/bin/sh".into()),
                home,
                exit_when_idle: false,
                desktop_idle: false,
            });
            keeper.serve(listener).await.unwrap();
        });
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.join("shedd.sock").exists() {
        assert!(Instant::now() < deadline, "the keeper did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Attach {
    child: Child,
    out: mpsc::Receiver<Vec<u8>>,
    seen: String,
}

impl Attach {
    fn start(dir: &Path, id: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_shedd"))
            .args(["attach", id])
            .env("SHED_RUNTIME_DIR", dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, out) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self { child, out, seen: String::new() }
    }

    fn write(&mut self, bytes: &[u8]) {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn until(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.seen.contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.out.recv_timeout(left) {
                Ok(bytes) => self.seen.push_str(&String::from_utf8_lossy(&bytes)),
                Err(_) => panic!("{needle:?} did not come; output: {:?}", self.seen),
            }
        }
    }

    fn ended(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "attach did not end; output: {:?}", self.seen);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn exit_code(mut self) -> i32 {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "attach did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_remote_shell_outlives_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    start_keeper(dir.path());

    let mut first = Attach::start(dir.path(), "tab-1");
    first.write(b"echo remote-$((6*7))\n");
    first.until("remote-42");
    // Letting go (ssh ending) keeps the shell.
    assert_eq!(first.exit_code(), shed_keeper::attach::EXIT_DETACHED);

    // Joining again shows it again, then works as before.
    let mut second = Attach::start(dir.path(), "tab-1");
    second.until("remote-42");
    second.write(b"echo again-$((5*5))\n");
    second.until("again-25");

    // The tab closing: the shell goes too.
    second.write(shed_keeper::attach::CLOSE_REQUEST);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = second.child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(shed_keeper::attach::EXIT_ENDED));
            break;
        }
        assert!(Instant::now() < deadline, "closing did not end the shell");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Another id is another shell.
    let mut other = Attach::start(dir.path(), "tab-2");
    other.write(b"exit\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = other.child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(shed_keeper::attach::EXIT_ENDED), "a shell that exits ends the attach");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn list(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_shedd"))
        .arg("list")
        .env("SHED_RUNTIME_DIR", dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn two_pcs_share_a_remote_shell() {
    let dir = tempfile::tempdir().unwrap();
    start_keeper(dir.path());
    assert_eq!(list(dir.path()), "", "no shells yet");

    let mut one = Attach::start(dir.path(), "shared-1");
    one.write(b"echo ready-$((2*3))\n");
    one.until("ready-6");
    // Another PC joins the same shell: it sees what is there, and both see
    // what either types.
    let mut two = Attach::start(dir.path(), "shared-1");
    two.until("ready-6");
    two.write(b"echo from-two-$((3*3))\n");
    two.until("from-two-9");
    one.until("from-two-9");

    // The list says where it is and how many PCs show it.
    let listed = list(dir.path());
    let fields: Vec<&str> = listed.trim_end_matches('\n').split('\t').collect();
    assert_eq!(fields.len(), 7, "{listed:?}");
    assert_eq!((fields[0], fields[1], fields[2]), ("shared-1", "running", "2"), "{listed:?}");
    assert!(!fields[5].is_empty(), "its folder: {listed:?}");

    // One PC letting go leaves it to the other.
    assert_eq!(one.exit_code(), shed_keeper::attach::EXIT_DETACHED);
    two.write(b"echo still-$((4*4))\n");
    two.until("still-16");
    assert!(list(dir.path()).contains("\trunning\t1\t"));

    // Ending it from one PC ends it on every PC.
    let mut three = Attach::start(dir.path(), "shared-1");
    three.until("still-16");
    three.write(shed_keeper::attach::CLOSE_REQUEST);
    assert_eq!(three.ended(), shed_keeper::attach::EXIT_ENDED);
    assert_eq!(two.ended(), shed_keeper::attach::EXIT_ENDED, "the other PC lets go when the shell ends");
    assert_eq!(list(dir.path()), "");
}
