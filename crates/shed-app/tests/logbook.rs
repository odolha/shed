//! The logbook end to end: real bash with Shed's hook in a real keeper,
//! commands captured, and the history keys walking the right scope.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{Client, start_in};
use shed_logbook::{Context, Logbook, Scope};
use shed_model::TabId;
use shed_proto::{ClientMsg, Frame, KeeperMsg, ShowTarget};

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
    let dir = tempfile::tempdir().unwrap();
    let env = start_in(dir.path(), "shedd.sock");
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
