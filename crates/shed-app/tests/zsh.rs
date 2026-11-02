//! zsh with Shed's hook in a real keeper: prompts and commands marked,
//! the logbook kept, history walked, Shed's aliases applied. Skipped when
//! there is no zsh (`SHED_TEST_ZSH` can point at one).

mod common;

use std::time::{Duration, Instant};

use common::{Client, start_with_shell, zsh};
use shed_logbook::{Context, Logbook, Scope};
use shed_proto::{AliasTarget, ClientMsg, Frame, KeeperMsg};

#[test]
fn zsh_is_marked_logged_and_walked() {
    let Some(zsh) = zsh() else {
        eprintln!("no zsh: skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    // Without a ~/.zshrc, zsh would start its new-user wizard.
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    std::fs::write(dir.path().join("home/.zshrc"), "alias ll='ls -l'\n").unwrap();
    let env = start_with_shell(dir.path(), "shedd.sock", &zsh);
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Attach { bench: None });
    let (bench, tab, pane) = c.until("attached", |f, t| match f {
        Frame::Keeper(KeeperMsg::Attached { bench }) => {
            let tab = t.bench(*bench)?.tabs[0].clone();
            Some((*bench, tab.id, tab.active_pane))
        }
        _ => None,
    });
    c.prompt(pane);

    // Its own aliases are reported; Shed's are applied at the next prompt.
    c.until("shell aliases", |_, t| t.pane(pane).filter(|p| p.shell_aliases.contains_key("ll")).map(|_| ()));
    let aliases = [("hi".to_string(), "echo hello-from-shed".to_string())].into_iter().collect();
    c.msg(ClientMsg::SetAliases { level: AliasTarget::Tab(tab), aliases });
    std::thread::sleep(Duration::from_millis(300));
    c.input(pane, "\n");
    c.prompt(pane);
    c.input(pane, "hi\n");
    c.output_until(pane, "hello-from-shed");
    c.prompt(pane);

    c.input(pane, "echo two-$((1+1))\n");
    c.output_until(pane, "two-2");
    c.prompt(pane);
    c.input(pane, "false\n");
    c.prompt(pane);

    let here = Context { tab, bench };
    let deadline = Instant::now() + Duration::from_secs(10);
    let logged = loop {
        let book = Logbook::open_readonly(&env.logbook).ok();
        let got = book.map(|b| b.search(Scope::Local, here, "", None, 10).unwrap()).unwrap_or_default();
        if got.len() >= 3 || Instant::now() > deadline {
            break got;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let commands: Vec<&str> = logged.iter().map(|e| e.command.as_str()).collect();
    assert_eq!(commands, ["false", "echo two-$((1+1))", "hi"]);
    assert_eq!((logged[0].exit, logged[1].exit), (Some(1), Some(0)));

    // Ctrl+Up walks the bench's history.
    c.input(pane, "echo t");
    c.input(pane, "\x1b[1;5A");
    let walked = c.output_until(pane, "\x1b]7777;shed;walk;bench;1;");
    assert!(walked.contains("walk;bench;1;1;"), "{walked:?}");
    c.input(pane, "\n");
    c.output_until(pane, "133;C\x07");
    c.output_until(pane, "two-2");
}
