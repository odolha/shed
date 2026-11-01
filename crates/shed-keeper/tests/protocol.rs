//! An in-process keeper driven by a fake client over a real socket.

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shed_keeper::{Keeper, KeeperConfig};
use shed_model::{BenchId, PaneId, Shed};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, read_frame, write_frame};

struct Client {
    write: UnixStream,
    frames: mpsc::Receiver<Frame>,
}

impl Client {
    fn connect(socket: &Path) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => panic!("cannot connect: {e}"),
            }
        };
        let mut read = BufReader::new(stream.try_clone().unwrap());
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(frame) = read_frame(&mut read) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let mut client = Client { write: stream, frames };
        client.send(Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window }));
        client
    }

    fn send(&mut self, frame: Frame) {
        write_frame(&mut self.write, &frame).unwrap();
        self.write.flush().unwrap();
    }

    /// Waits for a frame matching `pick`, skipping others.
    fn expect<T>(&self, what: &str, mut pick: impl FnMut(Frame) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(left) {
                Ok(frame) => {
                    if let Some(v) = pick(frame) {
                        return v;
                    }
                }
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    /// Attaches to the default bench and returns it with its only pane.
    fn attach(&mut self) -> (BenchId, PaneId) {
        self.send(Frame::Client(ClientMsg::Attach { bench: None }));
        let tree: Shed = self.expect("tree", |f| match f {
            Frame::Keeper(KeeperMsg::TreeChanged(t)) => Some(t),
            _ => None,
        });
        let bench = self.expect("attached", |f| match f {
            Frame::Keeper(KeeperMsg::Attached { bench }) => Some(bench),
            _ => None,
        });
        let pane = tree.bench(bench).unwrap().tabs[0].active_pane;
        (bench, pane)
    }

    /// Collects output from `pane` until it contains `needle`.
    fn wait_output(&self, pane: PaneId, needle: &str) -> String {
        let mut seen = String::new();
        self.expect(needle, |f| match f {
            Frame::Output { pane: p, bytes } if p == pane => {
                seen.push_str(&String::from_utf8_lossy(&bytes));
                seen.contains(needle).then(|| seen.clone())
            }
            _ => None,
        })
    }
}

fn start_keeper(dir: &Path) -> std::path::PathBuf {
    let socket = dir.join("shedd.sock");
    let listener_path = socket.clone();
    let home = dir.to_path_buf();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&listener_path).unwrap();
            let keeper = Keeper::new(KeeperConfig {
                socket: listener_path,
                shell: Some("/bin/sh".into()),
                home,
                exit_when_idle: false,
            });
            keeper.serve(listener).await.unwrap();
        });
    });
    socket
}

#[test]
fn attach_input_output_resize_exit() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());
    let mut client = Client::connect(&socket);
    client.expect("welcome", |f| matches!(f, Frame::Keeper(KeeperMsg::Welcome { .. })).then_some(()));

    let (_bench, pane) = client.attach();

    client.send(Frame::Input { pane, bytes: b"echo shed-$((40+2))\n".to_vec() });
    client.wait_output(pane, "shed-42");

    client.send(Frame::Client(ClientMsg::Resize { pane, cols: 100, rows: 30, cell_width: 9, cell_height: 18 }));
    client.send(Frame::Input { pane, bytes: b"stty size\n".to_vec() });
    client.wait_output(pane, "30 100");

    // The keeper's own emulator answers terminal queries, even with no window drawing.
    let query = "stty -icanon -echo min 0 time 5; printf '\\033[6n'; od -c; stty sane\n";
    client.send(Frame::Input { pane, bytes: query.as_bytes().to_vec() });
    client.wait_output(pane, "R\r\n");

    client.send(Frame::Input { pane, bytes: b"exit 7\n".to_vec() });
    let code = client.expect("pane exit", |f| match f {
        Frame::Keeper(KeeperMsg::PaneExited { pane: p, code }) if p == pane => Some(code),
        _ => None,
    });
    assert_eq!(code, Some(7));
    let tree = client.expect("tree after exit", |f| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) => Some(t),
        _ => None,
    });
    assert!(tree.benches.is_empty());
}

#[test]
fn tools_survive_detach_and_reattach() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());

    let mut first = Client::connect(&socket);
    let (bench, pane) = first.attach();
    first.send(Frame::Input { pane, bytes: b"MARK=kept-alive\n".to_vec() });
    first.send(Frame::Input { pane, bytes: b"echo ready\n".to_vec() });
    first.wait_output(pane, "ready");
    drop(first);

    let mut second = Client::connect(&socket);
    let (bench2, pane2) = second.attach();
    assert_eq!((bench2, pane2), (bench, pane));
    second.send(Frame::Input { pane, bytes: b"echo $MARK\n".to_vec() });
    second.wait_output(pane, "kept-alive");
}
