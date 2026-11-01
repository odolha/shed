//! The logbook end to end: real bash with Shed's hook in a real keeper,
//! commands captured, and the history keys walking the right scope.

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shed_keeper::{Keeper, KeeperConfig};
use shed_logbook::{Context, Logbook, Scope};
use shed_model::{PaneId, Shed, TabId};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, ShowTarget, read_frame, write_frame};

struct Client {
    write: UnixStream,
    frames: mpsc::Receiver<Frame>,
    tree: Shed,
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
        let mut c = Client { write: stream, frames, tree: Shed::default() };
        c.msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window });
        c
    }

    fn msg(&mut self, msg: ClientMsg) {
        write_frame(&mut self.write, &Frame::Client(msg)).unwrap();
        self.write.flush().unwrap();
    }

    fn input(&mut self, pane: PaneId, text: &str) {
        write_frame(&mut self.write, &Frame::Input { pane, bytes: text.as_bytes().to_vec() }).unwrap();
        self.write.flush().unwrap();
    }

    /// Reads frames until `pick` accepts one, keeping the latest tree.
    fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&Frame, &Shed) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = self.frames.recv_timeout(left).unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            if let Frame::Keeper(KeeperMsg::TreeChanged(t)) = &frame {
                self.tree = t.clone();
            }
            if let Some(v) = pick(&frame, &self.tree) {
                return v;
            }
        }
    }

    /// Output of `pane` until it contains `needle`; returns all of it.
    fn output_until(&mut self, pane: PaneId, needle: &str) -> String {
        let mut seen = String::new();
        self.until(needle, |f, _| match f {
            Frame::Output { pane: p, bytes } if *p == pane => {
                seen.push_str(&String::from_utf8_lossy(bytes));
                seen.contains(needle).then(|| seen.clone())
            }
            _ => None,
        })
    }

    /// Waits for a fresh prompt in `pane` (the hook's "input starts" mark).
    fn prompt(&mut self, pane: PaneId) {
        self.output_until(pane, "\x1b]133;B");
    }
}

struct Env {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    logbook: PathBuf,
}

fn start() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let socket = dir.path().join("shedd.sock");
    let logbook = dir.path().join("state/logbook.sqlite");
    let cfg = KeeperConfig {
        socket: socket.clone(),
        config_file: dir.path().join("shed.toml"),
        shell: Some("/bin/bash".into()),
        home: home.clone(),
        exit_when_idle: false,
        logbook: Some(logbook.clone()),
        state_dir: dir.path().join("state"),
        shed_bin: Some(env!("CARGO_BIN_EXE_shed").into()),
        env: vec![
            ("HOME".into(), home.into()),
            ("HISTCONTROL".into(), "ignoreboth".into()),
            ("PS1".into(), "$ ".into()),
        ],
    };
    let listen = socket.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&listen).unwrap();
            Keeper::new(cfg).serve(listener).await.unwrap();
        });
    });
    Env { _dir: dir, socket, logbook }
}

fn walk_all(book: &Logbook, scope: Scope, cx: Context) -> Vec<String> {
    let total = book.walk(scope, cx, "", 1).unwrap().map_or(0, |w| w.total);
    (1..=total).map(|p| book.walk(scope, cx, "", p).unwrap().unwrap().command).collect()
}

/// Polls the logbook until `ok` holds for `scope` seen from `cx`.
fn logbook_until(path: &Path, scope: Scope, cx: Context, ok: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(book) = Logbook::open_readonly(path) {
            let got = walk_all(&book, scope, cx);
            if ok(&got) {
                return got;
            }
            if Instant::now() > deadline {
                panic!("logbook never matched; has {got:?}");
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

const CTRL_UP: &str = "\x1b[1;5A";
const CTRL_SHIFT_UP: &str = "\x1b[1;6A";
const UP: &str = "\x1b[A";

#[test]
fn bash_commands_are_logged_and_walked_by_scope() {
    let env = start();
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Attach { bench: None });
    let (bench, tab1, p1) = c.until("attached", |f, t| match f {
        Frame::Keeper(KeeperMsg::Attached { bench }) => {
            let tab = t.bench(*bench)?.tabs[0].clone();
            Some((*bench, tab.id, tab.active_pane))
        }
        _ => None,
    });
    c.prompt(p1);

    // Captured with exit codes; a leading space keeps a command out.
    c.input(p1, "echo one-$((1+1))\n");
    c.output_until(p1, "one-2");
    c.prompt(p1);
    c.input(p1, "false\n");
    c.prompt(p1);
    c.input(p1, " echo secret\n");
    c.prompt(p1);
    let here1 = Context { tab: tab1, bench };
    let local = logbook_until(&env.logbook, Scope::Local, here1, |l| l.len() >= 2);
    assert_eq!(local, ["false", "echo one-$((1+1))"]);
    let book = Logbook::open_readonly(&env.logbook).unwrap();
    let all = book.search(Scope::Local, here1, "", None, 10).unwrap();
    assert_eq!(all.len(), 2, "the leading-space command is not logged: {all:?}");
    assert_eq!(all[0].exit, Some(1));
    assert_eq!(all[1].exit, Some(0));
    // Re-running a command bash treats as a duplicate is not logged twice.
    c.input(p1, "false\n");
    c.prompt(p1);

    // A second tab in the same bench.
    c.msg(ClientMsg::NewTab { bench, cwd: None });
    let (tab2, p2) = c.until("second tab", |_, t| {
        let b = t.bench(bench)?;
        (b.tabs.len() == 2).then(|| (b.tabs[1].id, b.tabs[1].active_pane))
    });
    c.prompt(p2);
    c.input(p2, "echo from-tab-two\n");
    c.prompt(p2);
    logbook_until(&env.logbook, Scope::Local, Context { tab: tab2, bench }, |l| l.len() == 1);

    // Plain Up in tab two knows only tab two's command.
    c.input(p2, UP);
    c.input(p2, "\n");
    c.output_until(p2, "133;C\x07from-tab-two");
    c.prompt(p2);

    // Ctrl+Up in tab one, with "echo f" typed, finds tab two's command.
    c.input(p1, "echo f");
    c.input(p1, CTRL_UP);
    let walked = c.output_until(p1, "\x1b]7777;shed;walk;bench;1;");
    assert!(walked.contains("walk;bench;1;1;"), "{walked:?}");
    c.input(p1, "\n");
    c.output_until(p1, "133;C\x07from-tab-two");
    c.prompt(p1);

    // Another bench: Ctrl+Shift+Up walks everything, "fal" finds "false".
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("other".into()), cwd: None }));
    let other = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(other) });
    let p3 = c.until("other bench", |_, t| t.bench(other).map(|b| b.tabs[0].active_pane));
    c.prompt(p3);
    c.input(p3, "fal");
    c.input(p3, CTRL_SHIFT_UP);
    c.output_until(p3, "\x1b]7777;shed;walk;global;1;");
    c.input(p3, "\n");
    c.prompt(p3);
    let global = logbook_until(&env.logbook, Scope::Bench, Context { tab: TabId::new(), bench: other }, |l| !l.is_empty());
    assert_eq!(global, ["false"], "the walked command ran in the other bench");
}
