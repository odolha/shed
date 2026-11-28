//! Remote tabs shared between PCs, against a stand-in for ssh that runs the
//! "server" locally (its own home and Shed folders): a second tab joins the
//! shell the first one opened, closing one tab keeps the shell, ending it
//! ends it everywhere, its commands are kept on the server only, and
//! Reconnect joins it again keeping its state.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::{Client, start_in};
use shed_model::{BenchId, PaneId, Shed, TabId};
use shed_proto::{ClientMsg, Frame, KeeperMsg, ShowTarget};

fn tree_until(c: &mut Client, what: &str, ok: impl Fn(&Shed) -> bool) -> Shed {
    if ok(&c.tree) {
        return c.tree.clone();
    }
    c.until(what, |f, _| match f {
        Frame::Keeper(KeeperMsg::TreeChanged(t)) if ok(t) => Some(t.clone()),
        _ => None,
    })
}

struct Server {
    root: PathBuf,
    shedd: PathBuf,
}

impl Server {
    fn env(&self, c: &mut Command) {
        let r = &self.root;
        c.env("HOME", r).env("SHED_RUNTIME_DIR", r.join("run")).env("XDG_DATA_HOME", r.join("data"));
        c.env("XDG_STATE_HOME", r.join("state")).env("XDG_CONFIG_HOME", r.join("config"));
    }

    /// `shedd list` there: (id, attached) for each shell.
    fn shells(&self) -> Vec<(String, u32)> {
        let mut c = Command::new(&self.shedd);
        c.arg("list");
        self.env(&mut c);
        let out = c.output().unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f[0].to_string(), f[2].parse().unwrap())
            })
            .collect()
    }

    fn shells_until(&self, what: &str, ok: impl Fn(&[(String, u32)]) -> bool) -> Vec<(String, u32)> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let shells = self.shells();
            if ok(&shells) {
                return shells;
            }
            assert!(Instant::now() < deadline, "server shells never {what}: {shells:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stops the server's keeper (started by `shedd attach`, detached).
    fn stop(&self) {
        let mut c = Command::new(self.shedd.with_file_name("shed"));
        c.arg("stop");
        self.env(&mut c);
        let _ = c.output();
    }
}

/// The stand-in ssh: it writes each command line to `ssh.log` beside it, and
/// has no shared connections (`-O`, `-G` fail as with none).
fn fake_ssh(dir: &Path, server: &Path) -> PathBuf {
    let script = dir.join("fakessh");
    let r = server.display();
    let log = dir.join("ssh.log");
    let log = log.display();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             echo \"$*\" >> {log}\n\
             case \" $* \" in *\" -O \"*|*\" -G \"*) exit 255;; esac\n\
             while [ $# -gt 0 ]; do case \"$1\" in -o) shift 2;; -*) shift;; *) break;; esac; done\n\
             shift\n\
             while [ $# -gt 0 ]; do case \"$1\" in -o) shift 2;; -*) shift;; *) break;; esac; done\n\
             export HOME={r} SHED_RUNTIME_DIR={r}/run XDG_DATA_HOME={r}/data XDG_STATE_HOME={r}/state XDG_CONFIG_HOME={r}/config\n\
             cd {r} && exec sh -c \"$*\"\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn remote_tab(c: &mut Client, bench: BenchId, join: Option<String>, but: &[TabId]) -> (TabId, PaneId, String) {
    c.msg(ClientMsg::NewRemoteTab { bench, host: "srv".into(), join });
    let tree = tree_until(c, "a remote tab", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().any(|t| t.remote_info().is_some() && !but.contains(&t.id))));
    let tab = tree.bench(bench).unwrap().tabs.iter().find(|t| t.remote_info().is_some() && !but.contains(&t.id)).unwrap();
    (tab.id, tab.panes[0].id, tab.remote_info().unwrap().id.clone())
}

#[test]
fn two_tabs_share_a_remote_shell() {
    let shedd = PathBuf::from(env!("CARGO_BIN_EXE_shed")).with_file_name("shedd");
    if !shedd.is_file() {
        eprintln!("skipped: build shedd first (cargo test --workspace does)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let server = Server { root: dir.path().join("server"), shedd: dir.path().join("server/.local/bin/shedd") };
    std::fs::create_dir_all(&server.root).unwrap();
    // The panes this keeper starts take these.
    // SAFETY: set before the keeper (and its threads) start; this file has one test.
    unsafe {
        std::env::set_var("SHED_SSH", fake_ssh(dir.path(), &server.root));
        std::env::set_var("SHED_REMOTE_SHEDD", &shedd);
        // `shed remote connect` notes the hosts it joins there, not in your own state.
        std::env::set_var("XDG_STATE_HOME", dir.path().join("pc-state"));
    }
    let env = start_in(dir.path(), "local.sock");
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Show(ShowTarget::NewBench { name: Some("work".into()), cwd: None }));
    let bench = c.until("show", |f, _| match f {
        Frame::Keeper(KeeperMsg::Show { bench }) => Some(*bench),
        _ => None,
    });
    c.msg(ClientMsg::Attach { bench: Some(bench) });

    // A new shell: shedd is put on the server, and the shell starts there.
    let (first_tab, first, id) = remote_tab(&mut c, bench, None, &[]);
    c.seen(first, "\x1bc");
    c.input(first, "echo shared-$((6*7))\r");
    c.output_until(first, "shared-42");
    assert!(server.shedd.is_file(), "shedd was put on the server");
    server.shells_until("showed the first tab", |s| s == [(id.clone(), 1)]);

    // Another PC joining it (here: another tab) sees what is there.
    let (second_tab, second, joined) = remote_tab(&mut c, bench, Some(id.clone()), &[first_tab]);
    assert_eq!(joined, id);
    c.seen(second, "shared-42");
    server.shells_until("showed both tabs", |s| s == [(id.clone(), 2)]);
    c.input(second, "echo both-$((5*5))\r");
    c.output_until(first, "both-25");

    // Closing a tab lets go of the shell; it keeps running for the other.
    c.msg(ClientMsg::CloseTab { tab: second_tab });
    tree_until(&mut c, "the second tab gone", |t| t.tab(second_tab).is_none());
    server.shells_until("let go of the second tab", |s| s == [(id.clone(), 1)]);
    c.input(first, "echo still-$((4*4))\r");
    c.output_until(first, "still-16");

    // Reconnect drops the connection and joins again at once; the shell on
    // the server kept what it had.
    c.input(first, "kept=v$((3*3)); echo set-$kept\r");
    c.output_until(first, "set-v9");
    let log = dir.path().join("ssh.log");
    let attaches = || std::fs::read_to_string(&log).unwrap().lines().filter(|l| l.contains(&format!("attach {id}"))).count();
    let before = attaches();
    c.msg(ClientMsg::ReconnectRemote { tab: first_tab });
    c.output_until(first, "Reconnecting to srv");
    c.output_until(first, "\x1bc");
    assert_eq!(attaches(), before + 1, "joined again");
    c.input(first, "echo got-$kept\r");
    c.output_until(first, "got-v9");
    server.shells_until("showed the first tab again", |s| s == [(id.clone(), 1)]);

    // Its commands are in the server's logbook, not this one.
    let local = shed_logbook::Logbook::open_readonly(&env.logbook).ok();
    let here = shed_logbook::Context { tab: first_tab, bench };
    let found = local.map(|b| b.search(shed_logbook::Scope::Global, here, "shared-", None, 10).unwrap()).unwrap_or_default();
    assert!(found.is_empty(), "recorded locally: {found:?}");
    let book = shed_logbook::Logbook::open_readonly(&server.root.join("state/shed/logbook.sqlite")).unwrap();
    let there = book.search(shed_logbook::Scope::Global, here, "shared-", None, 10).unwrap();
    assert_eq!(there.len(), 1, "recorded on the server");

    // A duplicate is a shell of its own on the same host, beside it.
    c.msg(ClientMsg::DuplicateTab { tab: first_tab, after: true });
    let tree = tree_until(&mut c, "a duplicate", |t| t.bench(bench).is_some_and(|b| b.tabs.iter().filter(|t| t.remote_info().is_some()).count() == 2));
    let tabs = &tree.bench(bench).unwrap().tabs;
    let at = tabs.iter().position(|t| t.id == first_tab).unwrap();
    let dup = tabs[at + 1].clone();
    let own = dup.remote_info().expect("a remote tab");
    assert_eq!(own.host, "srv");
    assert_ne!(own.id, id);
    let dup_pane = dup.panes[0].id;
    c.seen(dup_pane, "\x1bc");
    c.input(dup_pane, "echo own-${kept:-none}\r");
    c.output_until(dup_pane, "own-none");
    server.shells_until("a shell for the duplicate", |s| s.len() == 2 && s.contains(&(own.id.clone(), 1)));
    c.msg(ClientMsg::EndRemoteTab { tab: dup.id });
    tree_until(&mut c, "the duplicate gone", |t| t.tab(dup.id).is_none());
    server.shells_until("ended the duplicate's shell", |s| s == [(id.clone(), 1)]);

    // Ending it ends it on the server, and the tab goes.
    c.msg(ClientMsg::EndRemoteTab { tab: first_tab });
    tree_until(&mut c, "the first tab gone", |t| t.tab(first_tab).is_none());
    server.shells_until("ended the shell", |s| s.is_empty());
    server.stop();
}
