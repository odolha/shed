//! Following a project on disk: a watch per folder (not ignored ones, so a
//! big `node_modules` costs nothing), changes handed on in batches.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::files::Excluder;

/// Changes that come this close together are handed on together.
const SETTLE: Duration = Duration::from_millis(40);

pub struct Watch {
    watcher: Arc<Mutex<RecommendedWatcher>>,
    /// Folders could not all be watched (the system's limit); changes there
    /// show only on refresh.
    pub limited: bool,
}

impl Watch {
    /// Watches `dirs` (and `.git` under the first, the root) and calls
    /// `changed` with each batch of changed paths, from another thread.
    /// New folders are watched as they appear, unless `excluder` leaves them out.
    pub fn new(dirs: &[PathBuf], excluder: Excluder, changed: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            use notify::EventKind::{Create, Modify, Remove};
            use notify::event::ModifyKind;
            // Reads and opens are not changes (the walk itself opens every `.gitignore`).
            if let Ok(event) = event
                && matches!(event.kind, Create(_) | Remove(_) | Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any))
            {
                for path in event.paths {
                    let _ = tx.send(path);
                }
            }
        })?;
        let watcher = Arc::new(Mutex::new(watcher));
        let mut limited = false;
        {
            let mut w = watcher.lock().unwrap();
            for dir in dirs {
                if w.watch(dir, RecursiveMode::NonRecursive).is_err() {
                    limited = true;
                }
            }
            if let Some(root) = dirs.first() {
                for git in [root.join(".git"), root.join(".git/refs/heads")] {
                    if git.is_dir() {
                        let _ = w.watch(&git, RecursiveMode::NonRecursive);
                    }
                }
            }
        }
        let watched: HashSet<PathBuf> = dirs.iter().cloned().collect();
        let weak = Arc::downgrade(&watcher);
        std::thread::Builder::new().name("project-watch".into()).spawn(move || {
            let mut watched = watched;
            // Ends when the watcher (and with it the sender) is dropped.
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                while let Ok(more) = rx.recv_timeout(SETTLE) {
                    batch.push(more);
                }
                batch.sort();
                batch.dedup();
                if let Some(watcher) = weak.upgrade() {
                    let mut w = watcher.lock().unwrap();
                    for path in &batch {
                        if path.is_dir() && !watched.contains(path) && !excluder.excludes(path, true) {
                            for dir in crate::files::walk(path, &[]).dirs {
                                if !excluder.excludes(&dir, true) && w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                                    watched.insert(dir);
                                }
                            }
                        }
                    }
                }
                changed(batch);
            }
        })?;
        Ok(Self { watcher, limited })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // The watcher stops with its last reference; nothing else to do.
        let _ = &self.watcher;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_arrive_and_new_folders_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let walked = crate::files::walk(&root, &[]);
        let _watch = Watch::new(&walked.dirs, Excluder::new(&root, &[]), move |batch| {
            let _ = tx.send(batch);
        })
        .unwrap();
        std::fs::write(root.join("a.txt"), "1").unwrap();
        let batch = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(batch.contains(&root.join("a.txt")), "{batch:?}");
        // Reading is not a change.
        let _ = std::fs::read(root.join("a.txt"));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "a read was passed on");
        std::fs::create_dir(root.join("sub")).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(root.join("sub/b.txt"), "2").unwrap();
        let mut seen = false;
        while let Ok(batch) = rx.recv_timeout(Duration::from_secs(5)) {
            if batch.contains(&root.join("sub/b.txt")) {
                seen = true;
                break;
            }
        }
        assert!(seen, "a new folder is watched");
    }
}
