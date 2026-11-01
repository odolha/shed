//! An in-process keeper driven by a fake client over a real socket.

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shed_keeper::{Keeper, KeeperConfig};
use shed_emu::{AlacrittyEmulator, Emulator, GridSize, Palette};
use shed_model::{BenchColor, BenchId, PaneId, Shed, SplitDir};
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
                config_file: home.join("shed.toml"),
                logbook: Some(home.join("logbook.sqlite")),
                state_dir: home.join("state"),
                shed_bin: None,
                env: Vec::new(),
                store: None,
                desktop: None,
                aliases_file: home.join("aliases.toml"),
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

#[test]
fn binary_output_does_not_type_into_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    // Binary with DECID (ESC Z) and DA (CSI c) queries buried in it.
    let mut blob = Vec::new();
    for i in 0..2000u32 {
        blob.extend_from_slice(&[0x89, 0x00, (i % 251) as u8, 0xfe, 0x1b, b'Z', 0xc3, 0x28, 0x1b, b'[', b'c']);
    }
    std::fs::write(dir.path().join("blob.bin"), &blob).unwrap();

    let socket = start_keeper(dir.path());
    let mut client = Client::connect(&socket);
    let (_bench, pane) = client.attach();
    client.send(Frame::Input { pane, bytes: b"cat blob.bin; echo; echo cat-done\n".to_vec() });
    client.wait_output(pane, "cat-done");
    // Anything typed into the shell's input now would be read by `head`. The
    // markers are split in the command so its echo does not match them.
    let probe = "stty -icanon -echo min 0 time 3; printf 'input''->'; head -c 100 | od -An -c | tr -d ' \\n'; echo '<-''input'; stty sane\n";
    client.send(Frame::Input { pane, bytes: probe.as_bytes().to_vec() });
    let out = client.wait_output(pane, "<-input");
    let typed = &out[out.rfind("input->").unwrap() + "input->".len()..out.rfind("<-input").unwrap()];
    assert_eq!(typed, "", "replies leaked into the shell");
}

/// A window attaching mid-session must end up with the same screen as one
/// that watched everything, full-screen apps included.
#[test]
fn reattach_replays_the_screen_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());

    let mut watcher = Client::connect(&socket);
    let (_bench, pane) = watcher.attach();
    let mut seen = AlacrittyEmulator::new(GridSize::new(80, 24), Palette::default());
    let feed_until = |client: &Client, emu: &mut AlacrittyEmulator, needle: &str| {
        let mut text = String::new();
        client.expect(needle, |f| match f {
            Frame::Output { pane: p, bytes } if p == pane => {
                emu.advance(&bytes);
                text.push_str(&String::from_utf8_lossy(&bytes));
                text.contains(needle).then_some(())
            }
            _ => None,
        });
    };
    feed_until(&watcher, &mut seen, "$ ");

    // Scrollback, colours, then a full-screen app with its own modes and region.
    let setup = "for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do printf '\\033[3%dmline %s\\033[0m\\n' $((i % 8)) $i; done; \
        printf '\\033[?1049h\\033[?1h\\033[2;20r\\033[H\\033[7m status \\033[0m\\033[10;5Hin the app'; echo; echo setup''-done\n";
    watcher.send(Frame::Input { pane, bytes: setup.as_bytes().to_vec() });
    feed_until(&watcher, &mut seen, "setup-done");

    let mut late = Client::connect(&socket);
    late.send(Frame::Client(ClientMsg::Attach { bench: None }));
    let (cols, rows, replay) = late.expect("replay", |f| match f {
        Frame::Keeper(KeeperMsg::Replay { pane: p, cols, rows, bytes }) if p == pane => Some((cols, rows, bytes)),
        _ => None,
    });
    let mut restored = AlacrittyEmulator::new(GridSize::new(cols, rows), Palette::default());
    restored.advance(&replay);

    // Leave the alt screen and print more; both windows see it live.
    watcher.send(Frame::Input { pane, bytes: b"printf '\\033[r\\033[?1049l'; echo after''-app\n".to_vec() });
    feed_until(&watcher, &mut seen, "after-app");
    feed_until(&late, &mut restored, "after-app");
    // Let any trailing prompt arrive at both.
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(Frame::Output { bytes, .. }) = watcher.frames.try_recv() {
        seen.advance(&bytes);
    }
    while let Ok(Frame::Output { bytes, .. }) = late.frames.try_recv() {
        restored.advance(&bytes);
    }

    assert_eq!(restored.dump_text(), seen.dump_text());
    assert!(restored.dump_text().contains("line 30"), "primary screen came back:\n{}", restored.dump_text());
    assert_eq!(restored.modes(), seen.modes());
    seen.scroll_display(i32::MAX);
    restored.scroll_display(i32::MAX);
    assert_eq!(restored.display_offset(), seen.display_offset(), "scrollback length");
    assert_eq!(restored.dump_text(), seen.dump_text(), "oldest scrollback page");
}

impl Client {
    fn tree_where(&self, what: &str, mut ok: impl FnMut(&Shed) -> bool) -> Shed {
        self.expect(what, |f| match f {
            Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(&t) => Some(t),
            _ => None,
        })
    }

    fn msg(&mut self, msg: ClientMsg) {
        self.send(Frame::Client(msg));
    }
}

#[test]
fn tabs_splits_focus_and_close() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());
    let mut c = Client::connect(&socket);
    let (bench, first) = c.attach();

    c.msg(ClientMsg::NewTab { bench, cwd: None });
    let tree = c.tree_where("second tab", |t| t.bench(bench).is_some_and(|b| b.tabs.len() == 2));
    let b = tree.bench(bench).unwrap();
    let second_tab = b.tabs[1].id;
    assert_eq!(b.active_tab, second_tab, "a new tab becomes active");

    c.msg(ClientMsg::Split { pane: first, dir: SplitDir::Right, cwd: Some("/tmp".into()) });
    let tree = c.tree_where("split", |t| t.bench(bench).is_some_and(|b| b.tabs[0].panes.len() == 2));
    let tab = &tree.bench(bench).unwrap().tabs[0];
    let new = tab.layout.leaves()[1];
    assert_eq!(tab.active_pane, new);
    assert_eq!(tab.pane(new).unwrap().cwd, std::path::Path::new("/tmp"));

    // The new pane runs a shell of its own.
    c.send(Frame::Input { pane: new, bytes: b"echo from-split\n".to_vec() });
    c.wait_output(new, "from-split");

    c.msg(ClientMsg::FocusPane { pane: first });
    c.tree_where("focus", |t| {
        t.bench(bench).is_some_and(|b| b.active_tab == b.tabs[0].id && b.tabs[0].active_pane == first)
    });

    c.msg(ClientMsg::ClosePane { pane: new });
    c.tree_where("pane closed", |t| t.bench(bench).is_some_and(|b| b.tabs[0].panes.len() == 1));

    c.msg(ClientMsg::CloseTab { tab: second_tab });
    c.tree_where("tab closed", |t| t.bench(bench).is_some_and(|b| b.tabs.len() == 1));
}

#[test]
fn rename_colour_move_and_tool_tracking() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());
    let mut c = Client::connect(&socket);
    let (bench, pane) = c.attach();

    c.msg(ClientMsg::RenameBench { bench, name: "backend".into() });
    c.msg(ClientMsg::SetBenchColor { bench, color: BenchColor::Coral });
    c.tree_where("rename", |t| t.bench(bench).is_some_and(|b| b.name == "backend" && b.color == BenchColor::Coral));

    // The foreground program shows up as the pane's tool, and goes away after.
    c.send(Frame::Input { pane, bytes: b"cd /tmp && sleep 3\n".to_vec() });
    let tree = c.tree_where("tool", |t| t.pane(pane).is_some_and(|p| p.tool.is_some()));
    let p = tree.pane(pane).unwrap();
    assert_eq!(p.tool.as_ref().unwrap().argv, vec!["sleep", "3"]);
    assert_eq!(p.cwd, std::path::Path::new("/tmp"));
    c.tree_where("tool gone", |t| t.pane(pane).is_some_and(|p| p.tool.is_none()));

    // Titles set by programs reach the tree.
    c.send(Frame::Input { pane, bytes: b"printf '\\033]2;my-title\\007'\n".to_vec() });
    c.tree_where("title", |t| t.pane(pane).is_some_and(|p| p.title.as_deref() == Some("my-title")));

    // Moving the only tab into another bench removes the first bench.
    c.msg(ClientMsg::Show(shed_proto::ShowTarget::NewBench { name: Some("other".into()), cwd: None }));
    let tree = c.tree_where("second bench", |t| t.benches.len() == 2);
    let other = tree.benches.iter().find(|b| b.name == "other").unwrap().id;
    let tab = tree.bench(bench).unwrap().tabs[0].id;
    c.msg(ClientMsg::MoveTab { tab, bench: other, index: 1 });
    let tree = c.tree_where("moved", |t| t.benches.len() == 1);
    assert_eq!(tree.benches[0].tabs[1].id, tab);
}

#[test]
fn show_goes_to_the_window_process() {
    let dir = tempfile::tempdir().unwrap();
    let socket = start_keeper(dir.path());
    let mut window = Client::connect(&socket);
    let (bench, _) = window.attach();

    // A second `shed`, as a CLI client, asks for a new bench.
    let mut cli = Client::connect(&socket);
    cli.msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Cli });
    cli.msg(ClientMsg::Show(shed_proto::ShowTarget::NewBench { name: None, cwd: None }));
    let shown = window.expect("show", |f| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(bench),
        _ => None,
    });
    assert_ne!(shown, bench);

    // Detached benches: the first is open in `window`, so only the new one is detached.
    cli.msg(ClientMsg::Show(shed_proto::ShowTarget::Detached));
    let again = window.expect("show detached", |f| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(bench),
        _ => None,
    });
    assert_eq!(again, shown);
}

#[test]
fn tag_rules_follow_tools() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("shed.toml"),
        "[[tag_rule]]\ntool = \"sleep 4*\"\ntag = \"napping\"\ncolor = \"teal\"\n",
    )
    .unwrap();
    let socket = start_keeper(dir.path());
    let mut c = Client::connect(&socket);
    let (bench, pane) = c.attach();
    c.send(Frame::Input { pane, bytes: b"sleep 4.5\n".to_vec() });
    let tree = c.tree_where("auto tag", |t| t.bench(bench).is_some_and(|b| !b.tabs[0].auto_tags.is_empty()));
    assert_eq!(tree.bench(bench).unwrap().tabs[0].auto_tags[0].label, "napping");
    c.tree_where("auto tag gone", |t| t.bench(bench).is_some_and(|b| b.tabs[0].auto_tags.is_empty()));
}
