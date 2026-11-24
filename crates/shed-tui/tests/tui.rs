//! The terminal interface against a real keeper (in this process, with real
//! bash), driven through its events and drawn on ratatui's test backend:
//! no terminal is touched.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use shed_keeper::{Keeper, KeeperConfig};
use shed_proto::{ClientKind, ShowTarget};
use shed_tui::{App, Event, Options, Screen};

fn bash() -> PathBuf {
    if let Some(path) = std::env::var_os("SHED_TEST_BASH") {
        return path.into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|d| d.join("bash")).find(|p| p.is_file()).unwrap_or_else(|| "/bin/bash".into())
}

/// A keeper with real bash in `dir`; returns its socket.
fn start_keeper(dir: &Path) -> PathBuf {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let socket = dir.join("keeper.sock");
    let cfg = KeeperConfig {
        socket: socket.clone(),
        config_file: dir.join("shed.toml"),
        shell: Some(bash()),
        home: home.clone(),
        exit_when_idle: false,
        logbook: Some(dir.join("state/logbook.sqlite")),
        state_dir: dir.join("state"),
        shed_bin: None,
        env: vec![("HOME".into(), home.into()), ("PS1".into(), "$ ".into())],
        store: Some(shed_save::Store::new(dir.join("benches"))),
        desktop: None,
        aliases_file: dir.join("aliases.toml"),
        link_secret: None,
        link_log: dir.join("link.log"),
    };
    let listen = socket.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&listen).unwrap();
            Keeper::new(cfg).serve(listener).await.unwrap();
        });
    });
    socket
}

struct Harness {
    app: App,
    events: mpsc::Receiver<Event>,
    term: Terminal<TestBackend>,
}

impl Harness {
    fn new(dir: &Path) -> Harness {
        let socket = start_keeper(dir);
        let deadline = Instant::now() + Duration::from_secs(5);
        let (conn, frames) = loop {
            match shed_client::keeper_conn::connect_at(&socket, ClientKind::Tui) {
                Ok(c) => break c,
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => panic!("no keeper: {e}"),
            }
        };
        let (tx, events) = mpsc::channel();
        let forward = tx.clone();
        std::thread::spawn(move || {
            while let Ok(frame) = frames.recv_blocking() {
                if forward.send(Event::Keeper(Box::new(frame))).is_err() {
                    return;
                }
            }
        });
        let options = Options { config: Default::default(), logbook: dir.join("state/logbook.sqlite"), dark: true, no_color: false };
        let app = App::new(conn, tx, options);
        Harness { app, events, term: Terminal::new(TestBackend::new(100, 30)).unwrap() }
    }

    /// Handles events (drawing after each batch, as the terminal would)
    /// until `ok` holds.
    fn until(&mut self, what: &str, ok: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.draw();
            if ok(&self.app) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            if let Ok(ev) = self.events.recv_timeout(left.min(Duration::from_millis(200))) {
                self.app.handle(ev);
                while let Ok(ev) = self.events.try_recv() {
                    self.app.handle(ev);
                }
            }
        }
    }

    /// Handles events until the drawn screen shows `needle`.
    fn until_screen(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.draw();
            if self.screen_text().contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {needle:?} on:\n{}", self.screen_text());
            if let Ok(ev) = self.events.recv_timeout(Duration::from_millis(100)) {
                self.app.handle(ev);
                while let Ok(ev) = self.events.try_recv() {
                    self.app.handle(ev);
                }
            }
        }
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.term.draw(|f| app.render(f)).unwrap();
    }

    fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.app.handle(Event::Term(TermEvent::Key(KeyEvent::new(code, mods))));
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => self.key(KeyCode::Enter, KeyModifiers::NONE),
                c => self.key(KeyCode::Char(c), KeyModifiers::NONE),
            }
        }
    }

    fn screen_text(&self) -> String {
        let buf = self.term.backend().buffer();
        let area = buf.area;
        (0..area.height).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }
}

fn tabs(app: &App) -> usize {
    app.current_bench().map_or(0, |b| b.tabs.len())
}

#[test]
fn a_bench_a_new_tab_and_a_command_echoed() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut h = Harness::new(dir.path());

    // The keeper makes the bench and tells this client to show it.
    h.app.start(ShowTarget::NewBench { name: Some("tui".into()), cwd: Some(work.clone()) });
    h.until("the bench", |a| a.screen == Screen::Bench && tabs(a) == 1);

    // A new terminal tab: Ctrl+B, then c.
    h.key(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('c'), KeyModifiers::NONE);
    h.until("the second tab, shown", |a| tabs(a) == 2 && a.active_tab().is_some_and(|t| a.current_bench().unwrap().tabs[1].id == t.id));
    let pane = h.app.focused_pane().unwrap();
    h.until("its prompt", |a| a.pane_text(pane).is_some_and(|t| t.contains('$')));

    h.type_text("echo hi\n");
    h.until("hi on a line of its own", |a| a.pane_text(pane).is_some_and(|t| t.lines().any(|l| l.trim() == "hi")));

    // Drawn: the tab bar names the bench, the pane shows the output.
    h.draw();
    let screen = h.screen_text();
    assert!(screen.lines().next().unwrap().contains("tui"), "{screen}");
    assert!(screen.lines().any(|l| l.trim() == "hi"), "{screen}");

    // Split it: two panes side by side, a line between them.
    h.key(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('%'), KeyModifiers::NONE);
    h.until("the split", |a| a.active_tab().is_some_and(|t| t.panes.len() == 2));
    h.draw();
    assert!(h.screen_text().contains('│'));

    // The benches screen lists it; Esc goes back.
    h.key(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('s'), KeyModifiers::NONE);
    h.until("the benches", |a| a.screen == Screen::Benches);
    assert!(h.screen_text().contains("running · shown here"), "{}", h.screen_text());
    h.key(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(h.app.screen, Screen::Bench);

    // Leaving: C-b d quits the interface; the keeper keeps the bench.
    h.key(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(h.app.quit);
}

#[test]
fn a_project_tab_lists_files_and_opens_one_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("proj");
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::write(work.join("src/main.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
    std::fs::write(work.join("README.md"), "# proj\n").unwrap();
    let mut h = Harness::new(dir.path());
    h.app.start(ShowTarget::NewBench { name: Some("p".into()), cwd: Some(work.clone()) });
    h.until("the bench", |a| a.screen == Screen::Bench && tabs(a) == 1);
    let bench = h.app.current_bench().unwrap().id;
    h.app.send(shed_proto::ClientMsg::NewProjectTab { bench, root: work.clone() });
    h.until("the project tab", |a| a.active_tab().is_some_and(|t| t.project_info().is_some()));
    h.until_screen("src");
    h.until_screen("README.md");

    // Open by name: o, part of the name, Enter: the file shows.
    h.key(KeyCode::Char('o'), KeyModifiers::NONE);
    h.type_text("main");
    h.until_screen("src/main.rs");
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.until_screen("println!(\"hello\")");
}

#[test]
fn bookmarks_are_listed_and_gone_to() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    let text: String = (1..=80).map(|n| format!("line number {n}\n")).collect();
    std::fs::write(work.join("src/lib.rs"), text).unwrap();
    let mut h = Harness::new(dir.path());
    h.app.start(ShowTarget::NewBench { name: Some("marks".into()), cwd: Some(work.clone()) });
    h.until("the bench", |a| a.screen == Screen::Bench && tabs(a) == 1);
    let bench = h.app.current_bench().unwrap().id;
    let mark = shed_model::Bookmark::file(work.join("src/lib.rs"), Some(60));
    h.app.send(shed_proto::ClientMsg::AddBookmark { bench, bookmark: mark });
    h.until("bookmarked", |a| a.current_bench().is_some_and(|b| b.bookmarks.len() == 1));

    // C-b b lists them; choosing one opens a project tab for its file, at its line.
    h.key(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('b'), KeyModifiers::NONE);
    h.until_screen("lib.rs:60");
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.until("a project tab", |a| a.active_tab().is_some_and(|t| t.project_info().is_some()));
    h.until_screen("line number 60");

    // m in the file bookmarks the line marked (already a bookmark: not
    // twice), or the one at the top of the view.
    h.key(KeyCode::Char('m'), KeyModifiers::NONE);
    h.until_screen("bookmarked lib.rs:60");
    h.key(KeyCode::PageDown, KeyModifiers::NONE);
    h.key(KeyCode::Char('m'), KeyModifiers::NONE);
    h.until("bookmarked again", |a| a.current_bench().is_some_and(|b| b.bookmarks.len() == 2));
    let second = h.app.current_bench().unwrap().bookmarks[1].clone();
    assert!(matches!(second.kind, shed_model::BookmarkKind::File { line: Some(l), .. } if l > 60), "{second:?}");
}
