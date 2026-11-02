//! The AI prompt end to end: a line starting with `>` goes to the configured
//! command (a fake one here), run in the pane's folder, and its answer lands
//! on the prompt without running.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{Client, start_with_shell, zsh};
use shed_model::AiState;
use shed_proto::{ClientMsg, Frame, KeeperMsg};

fn asks_and_types_the_answer(shell: &Path) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    std::fs::write(dir.path().join("home/.zshrc"), "").unwrap();
    let asked = dir.path().join("asked.txt");
    // The fake AI keeps what it was told, and where, and answers with a command.
    let ai = format!("{{ pwd; cat; }} > {}; echo '```'; echo 'echo from-the-ai'; echo '```'", asked.display());
    std::fs::write(dir.path().join("shed.toml"), format!("[ai]\ncommand = {ai:?}\n")).unwrap();
    let env = start_with_shell(dir.path(), "shedd.sock", shell);
    let mut c = Client::connect(&env.socket);
    c.msg(ClientMsg::Attach { bench: None });
    let pane = c.until("attached", |f, t| match f {
        Frame::Keeper(KeeperMsg::Attached { bench }) => Some(t.bench(*bench)?.tabs[0].active_pane),
        _ => None,
    });
    c.prompt(pane);
    c.input(pane, "echo before\n");
    c.prompt(pane);

    c.input(pane, "> say something\r");
    c.until("asking", |_, t| t.pane(pane).filter(|p| matches!(p.ai, Some(AiState::Asking(_)))).map(|_| ()));
    c.until("answered", |_, t| t.pane(pane).filter(|p| p.ai.is_none()).map(|_| ()));
    let told = std::fs::read_to_string(&asked).unwrap();
    assert!(told.contains("Request: say something"), "{told}");
    assert!(told.contains("echo before"), "the tab's history goes along: {told}");
    let typed = c.output_until(pane, "echo from-the-ai");
    assert!(!typed.contains("from-the-ai\r\n"), "the answer must not run by itself");

    // It runs when the user presses Enter.
    c.input(pane, "\r");
    c.output_until(pane, "133;C\x07from-the-ai\r\n");

    // A failing AI says why, until the next key.
    let failing = "echo 'not logged in' >&2; exit 1";
    std::fs::write(dir.path().join("shed.toml"), format!("[ai]\ncommand = {failing:?}\n")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        c.input(pane, "> again\r");
        c.until("asking again", |_, t| t.pane(pane).filter(|p| matches!(p.ai, Some(AiState::Asking(_)))).map(|_| ()));
        let state = c.until("failed or answered", |_, t| match &t.pane(pane)?.ai {
            Some(AiState::Failed(why)) => Some(Some(why.clone())),
            None => Some(None),
            Some(AiState::Asking(_)) => None,
        });
        if let Some(why) = state {
            assert_eq!(why, "not logged in");
            break;
        }
        // The config is read again only every so often; the old command
        // answered, so its answer is cleared off the line.
        assert!(Instant::now() < deadline, "the new command was never used");
        c.input(pane, "\x05\x15");
        std::thread::sleep(Duration::from_millis(300));
    }
    c.input(pane, "x");
    c.until("cleared", |_, t| t.pane(pane).filter(|p| p.ai.is_none()).map(|_| ()));

    // Typed ahead while a command runs, the request still goes to the AI and
    // is never run as a redirection (which would make a file "ahead").
    c.input(pane, "\x05\x15sleep 0.5\r> ahead please\r");
    c.until("asking ahead", |_, t| t.pane(pane).filter(|p| p.ai == Some(AiState::Asking("ahead please".into()))).map(|_| ()));
    assert!(!dir.path().join("home/ahead").exists());
}

#[test]
fn bash_asks_the_ai() {
    asks_and_types_the_answer(Path::new("/bin/bash"));
}

#[test]
fn zsh_asks_the_ai() {
    match zsh() {
        Some(zsh) => asks_and_types_the_answer(&zsh),
        None => eprintln!("no zsh: skipped"),
    }
}
