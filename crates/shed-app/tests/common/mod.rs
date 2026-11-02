//! A fake window client and an in-process keeper running real bash, shared by
//! the end-to-end tests.
#![allow(dead_code)]

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use shed_keeper::{Keeper, KeeperConfig};
use shed_model::{PaneId, Shed};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, read_frame, write_frame};

pub struct Client {
    write: UnixStream,
    pub frames: mpsc::Receiver<Frame>,
    pub tree: Shed,
    /// Every pane's output so far, whichever wait read it.
    pub seen: std::collections::HashMap<PaneId, String>,
    /// How far into `seen` each pane's prompts have been waited for.
    prompted: std::collections::HashMap<PaneId, usize>,
    /// How far into `seen` `output_until` has read, per pane.
    read: std::collections::HashMap<PaneId, usize>,
}

impl Client {
    pub fn connect(socket: &Path) -> Self {
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
        let mut c = Client { write: stream, frames, tree: Shed::default(), seen: Default::default(), prompted: Default::default(), read: Default::default() };
        c.msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window });
        c
    }

    pub fn msg(&mut self, msg: ClientMsg) {
        write_frame(&mut self.write, &Frame::Client(msg)).unwrap();
        self.write.flush().unwrap();
    }

    pub fn input(&mut self, pane: PaneId, text: &str) {
        write_frame(&mut self.write, &Frame::Input { pane, bytes: text.as_bytes().to_vec() }).unwrap();
        self.write.flush().unwrap();
    }

    /// Reads frames until `pick` accepts one, keeping the latest tree.
    pub fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&Frame, &Shed) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(frame) = self.frames.recv_timeout(left) else {
                // The end of each pane's output, to see where it stopped.
                let tails: Vec<String> = self.seen.values().map(|s| format!("{:?}", &s[s.floor_char_boundary(s.len().saturating_sub(400))..])).collect();
                panic!("timed out waiting for {what}; output ended with:\n{}", tails.join("\n"));
            };
            if let Frame::Keeper(KeeperMsg::TreeChanged(t)) = &frame {
                self.tree = t.clone();
            }
            if let Frame::Output { pane, bytes } = &frame {
                self.seen.entry(*pane).or_default().push_str(&String::from_utf8_lossy(bytes));
            }
            if let Some(v) = pick(&frame, &self.tree) {
                return v;
            }
        }
    }

    /// Waits until `pane`'s output since the last call contains `needle`
    /// (which may have come during another wait); returns that output.
    pub fn output_until(&mut self, pane: PaneId, needle: &str) -> String {
        loop {
            let from = self.read.get(&pane).copied().unwrap_or(0);
            let seen = self.seen.get(&pane).map_or("", String::as_str);
            if let Some(at) = seen.get(from..).and_then(|rest| rest.find(needle)) {
                // Everything seen, as callers look past the match too.
                let out = seen[from..].to_string();
                self.read.insert(pane, from + at + needle.len());
                return out;
            }
            self.until(needle, |f, _| matches!(f, Frame::Output { pane: p, .. } if *p == pane).then_some(()));
        }
    }

    /// Waits until `pane` has shown `needle` at any point so far.
    pub fn seen(&mut self, pane: PaneId, needle: &str) {
        if self.seen.get(&pane).is_some_and(|s| s.contains(needle)) {
            return;
        }
        self.until(needle, |f, _| match f {
            Frame::Output { pane: p, .. } if *p == pane => Some(()),
            _ => None,
        });
        self.seen(pane, needle);
    }

    /// Waits for a fresh prompt in `pane`: a command's end mark, then an
    /// "input starts" mark, after the last prompt waited for. Readline's
    /// redraws repeat "input starts" without an end, and do not count; a
    /// prompt that arrived during another wait does.
    pub fn prompt(&mut self, pane: PaneId) {
        let fresh = |seen: &str, from: usize| {
            let rest = seen.get(from..)?;
            let end = rest.rfind("\x1b]133;D")?;
            let input = rest[end..].find("\x1b]133;B")?;
            Some(from + end + input + 1)
        };
        loop {
            let from = self.prompted.get(&pane).copied().unwrap_or(0);
            if let Some(at) = fresh(self.seen.get(&pane).map_or("", String::as_str), from) {
                self.prompted.insert(pane, at);
                return;
            }
            self.until("a prompt", |f, _| matches!(f, Frame::Output { pane: p, .. } if *p == pane).then_some(()));
        }
    }
}

pub struct Env {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub logbook: PathBuf,
}

/// Starts a keeper with real bash in `dir`, saving benches to `dir/benches`.
/// Each call gets its own socket, so a second keeper can follow a stopped one.
pub fn start_in(dir: &Path, socket_name: &str) -> Env {
    start_with_shell(dir, socket_name, &bash())
}

/// bash for the tests: `SHED_TEST_BASH`, or the first `bash` on PATH (on
/// macOS, Homebrew's bash 5 rather than the system's 3.2).
pub fn bash() -> PathBuf {
    if let Some(path) = std::env::var_os("SHED_TEST_BASH") {
        return path.into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|d| d.join("bash")).find(|p| p.is_file()).unwrap_or_else(|| "/bin/bash".into())
}

/// zsh for the tests: `SHED_TEST_ZSH`, or `zsh` on PATH.
pub fn zsh() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SHED_TEST_ZSH") {
        return Some(path.into());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("zsh")).find(|p| p.is_file())
}

/// [`start_in`] with another shell.
pub fn start_with_shell(dir: &Path, socket_name: &str, shell: &Path) -> Env {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let socket = dir.join(socket_name);
    let logbook = dir.join("state/logbook.sqlite");
    let cfg = KeeperConfig {
        socket: socket.clone(),
        config_file: dir.join("shed.toml"),
        shell: Some(shell.into()),
        home: home.clone(),
        exit_when_idle: false,
        logbook: Some(logbook.clone()),
        state_dir: dir.join("state"),
        shed_bin: Some(env!("CARGO_BIN_EXE_shed").into()),
        env: vec![
            ("HOME".into(), home.into()),
            ("HISTCONTROL".into(), "ignoreboth".into()),
            ("PS1".into(), "$ ".into()),
        ],
        store: Some(shed_save::Store::new(dir.join("benches"))),
        desktop: None,
        aliases_file: dir.join("aliases.toml"),
    };
    let listen = socket.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let listener = shed_keeper::bind_socket(&listen).unwrap();
            Keeper::new(cfg).serve(listener).await.unwrap();
        });
    });
    Env { dir: dir.to_path_buf(), socket, logbook }
}
