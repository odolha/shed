//! Saving and loading benches end to end: a keeper saves as it stops, and
//! the next keeper brings the bench back as a reboot would.

mod common;

use std::time::{Duration, Instant};

use common::{Client, start_in};
use shed_model::{BenchId, Shed, SplitDir};
use shed_proto::{ClientMsg, Frame, KeeperMsg, ShowTarget};

/// Waits for a tree that `ok` accepts, polling the keeper's broadcasts.
fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

/// Shows `target` as the window process and attaches to the bench shown.
fn show_and_attach(c: &mut Client, target: ShowTarget) -> BenchId {
    c.msg(ClientMsg::Show(target));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    bench
}

#[test]
fn a_bench_survives_the_keeper_stopping() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("shed.toml"),
        "[rack]\nauto = [\"tail -f\"]\nnever = [\"sleep 600\"]\n",
    )
    .unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    // ── First keeper: build a bench and leave things running.
    let first = start_in(dir.path(), "first.sock");
    let mut c = Client::connect(&first.socket);
    let bench = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("work".into()), cwd: Some(work.clone()) });
    let tree = tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    let tab1 = tree.bench(bench).unwrap().tabs[0].clone();
    let p1 = tab1.active_pane;
    c.prompt(p1);
    c.input(p1, "echo marker-$((6*7)) && tail -f /dev/null\n");
    c.output_until(p1, "marker-42");

    c.msg(ClientMsg::Split { pane: p1, dir: SplitDir::Down, cwd: Some(work.clone()) });
    let tree = tree_until(&mut c, "split", |t| t.bench(bench).is_some_and(|b| b.tabs[0].panes.len() == 2));
    let p2 = tree.bench(bench).unwrap().tabs[0].layout.leaves()[1];
    c.prompt(p2);
    c.input(p2, "sleep 300\n");

    c.msg(ClientMsg::NewTab { bench, cwd: Some(work.clone()) });
    let tree = tree_until(&mut c, "second tab", |t| t.bench(bench).is_some_and(|b| b.tabs.len() == 2));
    let tab2 = tree.bench(bench).unwrap().tabs[1].clone();
    let p3 = tab2.active_pane;
    c.prompt(p3);
    c.input(p3, "sleep 600\n");
    // Let the commands be marked as running.
    std::thread::sleep(Duration::from_millis(400));

    // Stopping saves everything.
    c.msg(ClientMsg::Shutdown);
    let file = dir.path().join("benches/work.toml");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&file).is_ok_and(|t| t.contains("sleep 600")) {
        assert!(Instant::now() < deadline, "the bench was not saved");
        std::thread::sleep(Duration::from_millis(50));
    }
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("tool = \"echo marker-$((6*7)) && tail -f /dev/null\""), "{saved}");
    assert!(saved.contains("layout = \"down 0.5 (0) (1)\""), "{saved}");
    drop(c);

    // ── Second keeper, as after a reboot: `shed` brings the session back.
    let second = start_in(dir.path(), "second.sock");
    let mut c = Client::connect(&second.socket);
    let shown = show_and_attach(&mut c, ShowTarget::Detached);
    assert_eq!(shown, bench, "the same bench, by id");

    let replay = c.until("replay of the first pane", |f, _| match f {
        Frame::Keeper(KeeperMsg::Replay { pane, bytes, .. }) if *pane == p1 => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    });
    assert!(replay.contains("marker-42"), "old scrollback is back");
    assert!(replay.contains("restored"), "under a divider");

    // `… && tail -f /dev/null` ends in a rack tool, so it restarts by itself;
    // `sleep 300` is not on the rack, so it is typed and waits for Enter;
    // `sleep 600` is on the never list, so nothing is typed.
    let tree = tree_until(&mut c, "restarted and waiting", |t| {
        t.pane(p1).and_then(|p| p.tool.as_ref()).is_some_and(|tool| tool.argv.first().is_some_and(|a| a.ends_with("tail")))
            && t.pane(p2).is_some_and(|p| p.waiting.is_some())
    });
    let b = tree.bench(bench).unwrap();
    assert_eq!(b.name, "work");
    assert_eq!(b.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![tab1.id, tab2.id]);
    assert_eq!(tree.pane(p1).unwrap().cwd, work);
    assert_eq!(tree.pane(p2).unwrap().waiting.as_deref(), Some("sleep 300"));
    assert_eq!(tree.pane(p3).unwrap().waiting, None);
    c.seen(p2, "sleep 300");

    // Pressing Enter runs the typed command, which clears its waiting state.
    c.input(p2, "\r");
    tree_until(&mut c, "sleep runs", |t| t.pane(p2).is_some_and(|p| p.waiting.is_none() && p.tool.is_some()));
}

#[test]
fn rack_tools_restart_by_themselves() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shed.toml"), "[rack]\nauto = [\"tail -f\"]\n").unwrap();

    let first = start_in(dir.path(), "first.sock");
    let mut c = Client::connect(&first.socket);
    let bench = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("logs".into()), cwd: None });
    let tree = tree_until(&mut c, "bench", |t| t.bench(bench).is_some());
    let pane = tree.bench(bench).unwrap().tabs[0].active_pane;
    c.prompt(pane);
    c.input(pane, "tail -f /dev/null\n");
    tree_until(&mut c, "tail runs", |t| t.pane(pane).is_some_and(|p| p.tool.is_some()));
    c.msg(ClientMsg::Shutdown);
    drop(c);

    let second = start_in(dir.path(), "second.sock");
    let mut c = Client::connect(&second.socket);
    // Loading by name, as a pinned launcher does.
    show_and_attach(&mut c, ShowTarget::Load { name: "logs".into() });
    let tree = tree_until(&mut c, "tail restarted", |t| {
        t.pane(pane).and_then(|p| p.tool.as_ref()).is_some_and(|tool| tool.argv.first().is_some_and(|a| a.ends_with("tail")))
    });
    assert_eq!(tree.pane(pane).unwrap().waiting, None);
}

#[test]
fn renaming_keeps_the_launcher_and_names_stay_apart() {
    let dir = tempfile::tempdir().unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let web = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("web".into()), cwd: None });
    let tree = tree_until(&mut c, "web", |t| t.bench(web).is_some());
    assert_eq!(tree.bench(web).unwrap().launcher, "web");

    c.msg(ClientMsg::RenameBench { bench: web, name: "shop".into() });
    let tree = tree_until(&mut c, "renamed", |t| t.bench(web).is_some_and(|b| b.name == "shop"));
    assert_eq!(tree.bench(web).unwrap().launcher, "web", "a pinned launcher keeps working");

    // A new bench may take the old name, but not the launcher.
    let second = show_and_attach(&mut c, ShowTarget::NewBench { name: Some("web".into()), cwd: None });
    let tree = tree_until(&mut c, "second", |t| t.bench(second).is_some());
    assert_eq!(tree.bench(second).unwrap().launcher, "web-2");

    // Saved with the bench; a copy gets its own.
    c.msg(ClientMsg::SaveBench { bench: web, as_name: Some("shop-copy".into()) });
    c.until("saved", |f, _| match f {
        Frame::Keeper(KeeperMsg::Done(text)) if text.starts_with("saved shop as") => Some(()),
        _ => None,
    });
    let saved = std::fs::read_to_string(dir.path().join("benches/shop.toml")).unwrap();
    assert!(saved.contains("launcher = \"web\""), "{saved}");
    let copy = std::fs::read_to_string(dir.path().join("benches/shop-copy.toml")).unwrap();
    assert!(copy.contains("launcher = \"shop-copy\""), "{copy}");
}
