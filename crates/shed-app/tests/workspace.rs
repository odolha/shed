//! A bench as a reproducible workspace: aliases at three levels, and each
//! pane's own startup and teardown.

mod common;

use std::time::{Duration, Instant};

use common::{Client, start_in};
use shed_model::{Aliases, BenchId, PaneId, PaneSetup, Shed};
use shed_proto::{AliasTarget, ClientMsg, Frame, KeeperMsg, PegboardInfo, ShowTarget};

fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

fn new_bench(c: &mut Client, name: &str, cwd: Option<std::path::PathBuf>) -> (BenchId, PaneId) {
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some(name.into()), cwd }));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    let tree = tree_until(c, "bench", |t| t.bench(bench).is_some());
    (bench, tree.bench(bench).unwrap().tabs[0].active_pane)
}

fn aliases(pairs: &[(&str, &str)]) -> Aliases {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Runs `ll` and returns which definition answered.
fn run_ll(c: &mut Client, pane: PaneId, expect: &str) {
    // An empty line first: the shell picks up alias changes at a prompt.
    c.input(pane, "\n");
    c.prompt(pane);
    c.input(pane, "ll\n");
    c.output_until(pane, &format!("133;C\x07{expect}"));
    c.prompt(pane);
}

#[test]
fn aliases_layer_over_the_shells_own() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    std::fs::write(dir.path().join("home/.bashrc"), "alias ll='echo shell-ll'\nalias only='echo shell-only'\n").unwrap();
    std::fs::write(dir.path().join("aliases.toml"), "ll = \"echo global-ll\"\n").unwrap();

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let (bench, pane) = new_bench(&mut c, "aliases", None);
    let tab = c.tree.bench(bench).unwrap().tabs[0].id;
    c.prompt(pane);

    run_ll(&mut c, pane, "global-ll");
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Bench(bench), aliases: aliases(&[("ll", "echo bench-ll")]) });
    run_ll(&mut c, pane, "bench-ll");
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Tab(tab), aliases: aliases(&[("ll", "echo tab-ll")]) });
    run_ll(&mut c, pane, "tab-ll");

    // Taking Shed's away brings the shell's own back.
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Tab(tab), aliases: Aliases::new() });
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Bench(bench), aliases: Aliases::new() });
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Global, aliases: Aliases::new() });
    run_ll(&mut c, pane, "shell-ll");
    c.input(pane, "only\n");
    c.output_until(pane, "133;C\x07shell-only");

    // The shell's own aliases are reported for listing.
    let tree = tree_until(&mut c, "shell aliases", |t| t.pane(pane).is_some_and(|p| p.shell_aliases.contains_key("only")));
    assert_eq!(tree.pane(pane).unwrap().shell_aliases["ll"], "echo shell-ll");
    assert!(std::fs::read_to_string(dir.path().join("aliases.toml")).unwrap().contains("Shed's global aliases"));
}

#[test]
fn closing_stops_tools_runs_teardown_and_loading_runs_startup() {
    let dir = tempfile::tempdir().unwrap();
    let (work, other) = (dir.path().join("work"), dir.path().join("other"));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let marker = dir.path().join("torn-down");

    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let (bench, pane) = new_bench(&mut c, "proj", Some(work.clone()));
    c.prompt(pane);
    let setup = PaneSetup {
        start_in: Some(other.clone()),
        startup: vec!["echo first-$((1+1))".into(), "echo second-$((2+2))".into()],
        wait: false,
        teardown: vec![format!("echo stopped > {}", marker.display())],
        ..Default::default()
    };
    c.msg(ClientMsg::SetPaneSetup { pane, setup: setup.clone() });
    tree_until(&mut c, "setup", |t| t.pane(pane).is_some_and(|p| p.setup == setup));

    // Something running, for the close to stop first.
    c.input(pane, "sleep 300\n");
    tree_until(&mut c, "sleep", |t| t.pane(pane).is_some_and(|p| p.tool.is_some()));

    c.msg(ClientMsg::CloseBench { bench });
    tree_until(&mut c, "closing", |t| t.bench(bench).is_some_and(|b| b.closing));
    tree_until(&mut c, "closed", |t| t.bench(bench).is_none());
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "stopped", "the teardown ran after the tool stopped");

    let saved = std::fs::read_to_string(dir.path().join("benches/proj.toml")).unwrap();
    assert!(saved.contains("startup = [") && saved.contains("teardown = ["), "{saved}");

    // Loading starts in `start_in` and runs the startup commands in order.
    c.msg(ClientMsg::Show(ShowTarget::Load { name: "proj".into() }));
    let loaded = c.until("load", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    assert_eq!(loaded, bench);
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    let out = c.output_until(pane, "second-4");
    assert!(out.find("first-2").unwrap() < out.find("second-4").unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let tree = tree_until(&mut c, "loaded", |t| t.pane(pane).is_some());
        if tree.pane(pane).unwrap().cwd == other {
            break;
        }
        assert!(Instant::now() < deadline, "the pane did not start in start_in");
        std::thread::sleep(Duration::from_millis(100));
        c.msg(ClientMsg::FocusPane { pane });
    }
}

fn pegboard(c: &mut Client) -> PegboardInfo {
    c.msg(ClientMsg::Pegboard);
    c.until("pegboard", |f, _| match f {
        Frame::Keeper(KeeperMsg::Pegboard(info)) => Some(info.clone()),
        _ => None,
    })
}

fn previews(info: &PegboardInfo, pane: PaneId, needle: &str) -> bool {
    info.previews.iter().any(|(p, lines)| *p == pane && lines.iter().any(|l| l.contains(needle)))
}

#[test]
fn the_pegboard_previews_running_and_stored_benches() {
    let dir = tempfile::tempdir().unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let mut c = Client::connect(&env.socket);
    let (bench, pane) = new_bench(&mut c, "board", None);
    c.prompt(pane);
    c.input(pane, "echo preview-$((6*7))\n");
    c.output_until(pane, "preview-42");
    c.prompt(pane);

    let info = pegboard(&mut c);
    assert!(previews(&info, pane, "preview-42"), "{info:?}");
    assert!(info.stored.is_empty());

    // Closed, it is stored, and the preview comes from its saved scrollback.
    c.msg(ClientMsg::CloseBench { bench });
    tree_until(&mut c, "closed", |t| t.bench(bench).is_none());
    let info = pegboard(&mut c);
    let stored = info.stored.iter().find(|s| s.bench.id == bench).expect("stored");
    assert_eq!(stored.bench.name, "board");
    assert!(previews(&info, pane, "preview-42"), "{info:?}");
}

#[test]
fn startup_waits_for_other_panes_to_be_ready() {
    let dir = tempfile::tempdir().unwrap();
    let env = start_in(dir.path(), "shedd.sock");
    let order = dir.path().join("order.txt");
    let log = |name: &str| format!("echo {name} >> {}", order.display());
    let ids: Vec<PaneId> = (0..5).map(|_| PaneId::new()).collect();
    let quote = |v: &[String]| v.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ");
    let tab = |name: &str, pane: PaneId, startup: &[String], extra: &str| {
        format!(
            "\n[[tabs]]\nid = \"{}\"\nname = \"{name}\"\nlayout = \"0\"\n\n[[tabs.panes]]\nid = \"{pane}\"\ncwd = \"~\"\nstartup = [{}]\n{extra}\n",
            shed_model::TabId::new(),
            quote(startup)
        )
    };
    let mut file = format!("name = \"stack\"\nid = \"{}\"\ncolor = \"blue\"\ncwd = \"~\"\nsaved = \"2026-11-02T00:00:00Z\"\n", BenchId::new());
    // docker is ready when its startup finishes; backend when it says so,
    // and keeps running; frontend waits for backend.
    file += &tab("docker", ids[0], &["sleep 0.5".into(), log("docker")], "");
    file += &tab("backend", ids[1], &[log("backend"), "echo 'server ready'; sleep 30".into()], &format!("after = [\"{}\"]\nready_when = \"server ready\"", ids[0]));
    file += &tab("frontend", ids[2], &[log("frontend")], &format!("after = [\"{}\"]", ids[1]));
    // One that fails holds what waits for it, until started by hand.
    file += &tab("broken", ids[3], &["false".into()], "");
    file += &tab("after-broken", ids[4], &[log("after-broken")], &format!("after = [\"{}\"]", ids[3]));
    std::fs::create_dir_all(dir.path().join("benches")).unwrap();
    std::fs::write(dir.path().join("benches/stack.toml"), file).unwrap();

    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::Load { name: "stack".into() }));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });
    let tree = tree_until(&mut c, "held", |t| t.pane(ids[2]).is_some_and(|p| p.held.is_some()));
    assert_eq!(tree.pane(ids[2]).unwrap().held, Some(shed_model::Held::Waiting(vec!["backend".into()])));

    let deadline = Instant::now() + Duration::from_secs(15);
    let lines = loop {
        let text = std::fs::read_to_string(&order).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(String::from).collect();
        if lines.len() >= 3 || Instant::now() > deadline {
            break lines;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(lines, ["docker", "backend", "frontend"], "each started after what it waits for");
    tree_until(&mut c, "failed", |t| t.pane(ids[4]).is_some_and(|p| p.held == Some(shed_model::Held::Failed("broken".into()))));
    tree_until(&mut c, "frontend released", |t| t.pane(ids[2]).is_some_and(|p| p.held.is_none()));

    c.msg(ClientMsg::StartNow { pane: ids[4] });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&order).unwrap_or_default().contains("after-broken") {
        assert!(Instant::now() < deadline, "Start Now did not start it");
        std::thread::sleep(Duration::from_millis(50));
    }
}
