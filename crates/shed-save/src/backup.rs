//! Copies of what you set up: `shed.toml`, the alias files, your themes,
//! every saved bench and the session, made once a day by the keeper (and on demand), so
//! a broken or lost file can be brought back. Scrollback is left out.
//!
//! A backup is a folder in `~/.local/share/shed/backups/`, named by when it
//! was made (`2026-11-13` for the day's, `2026-11-13-153012` for one made on
//! demand), with `config/` and `data/` inside, laid out as the originals.

use std::io;
use std::path::{Path, PathBuf};

/// How many backups are kept; the oldest go first.
pub const KEEP: usize = 14;

/// Where the originals are, and where backups go.
#[derive(Clone, Debug)]
pub struct Places {
    /// `~/.config/shed`
    pub config: PathBuf,
    /// `~/.local/share/shed`
    pub data: PathBuf,
    pub backups: PathBuf,
}

impl Default for Places {
    fn default() -> Self {
        let data = shed_model::paths::data_dir();
        Places { config: shed_model::paths::config_dir(), backups: data.join("backups"), data }
    }
}

/// The files a backup holds, relative to `config/` or `data/`.
fn files(places: &Places) -> Vec<(&'static str, PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let mut add = |kind: &'static str, root: &Path, rel: PathBuf| {
        if root.join(&rel).is_file() {
            out.push((kind, root.join(&rel), rel));
        }
    };
    for (kind, root) in [("config", &places.config), ("data", &places.data)] {
        if let Ok(dir) = std::fs::read_dir(root) {
            for e in dir.flatten() {
                let name = PathBuf::from(e.file_name());
                if matches!(name.extension().and_then(|x| x.to_str()), Some("toml" | "sh")) {
                    add(kind, root, name);
                }
            }
        }
    }
    // Saved benches, and your own themes.
    for (kind, root, sub) in [("data", &places.data, "benches"), ("config", &places.config, "themes")] {
        if let Ok(dir) = std::fs::read_dir(root.join(sub)) {
            for e in dir.flatten() {
                let name = PathBuf::from(e.file_name());
                if name.extension().and_then(|x| x.to_str()) == Some("toml") {
                    add(kind, root, Path::new(sub).join(name));
                }
            }
        }
    }
    out
}

/// Makes a backup called `name`; its folder. Written aside, then put in place
/// whole.
pub fn make(places: &Places, name: &str) -> io::Result<PathBuf> {
    let dest = places.backups.join(name);
    let part = places.backups.join(format!(".{name}.part"));
    let _ = std::fs::remove_dir_all(&part);
    for (kind, from, rel) in files(places) {
        let to = part.join(kind).join(rel);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(&from, &to)?;
    }
    std::fs::create_dir_all(&part)?;
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

/// Today's backup, unless there is one; then only the newest `KEEP` stay.
/// The new backup's folder, if one was made.
pub fn daily(places: &Places) -> io::Result<Option<PathBuf>> {
    let today = local_stamp(now(), false);
    if list(places).iter().any(|b| b.starts_with(&today)) {
        return Ok(None);
    }
    let made = make(places, &today)?;
    prune(places);
    Ok(Some(made))
}

/// A backup now, named by the second.
pub fn now_backup(places: &Places) -> io::Result<PathBuf> {
    let made = make(places, &local_stamp(now(), true))?;
    prune(places);
    Ok(made)
}

/// The backups there are, oldest first.
pub fn list(places: &Places) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&places.backups)
        .map(|d| d.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| !n.starts_with('.')).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn prune(places: &Places) {
    let names = list(places);
    for old in names.iter().take(names.len().saturating_sub(KEEP)) {
        let _ = std::fs::remove_dir_all(places.backups.join(old));
    }
}

/// Puts a backup's files back (after backing up what is there now, so a
/// restore can be undone). Saved benches made since stay; those in the
/// backup are as they were. The keeper must not be running.
pub fn restore(places: &Places, name: &str) -> io::Result<PathBuf> {
    let from = places.backups.join(name);
    if name.is_empty() || name.contains('/') || !from.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no backup called {name:?}")));
    }
    let before = make(places, &format!("{}-before-restore", local_stamp(now(), true)))?;
    for (kind, root) in [("config", &places.config), ("data", &places.data)] {
        copy_tree(&from.join(kind), root)?;
    }
    Ok(before)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let Ok(dir) = std::fs::read_dir(from) else { return Ok(()) };
    std::fs::create_dir_all(to)?;
    for e in dir.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            let tmp = dst.with_extension("restoring");
            std::fs::copy(&src, &tmp)?;
            std::fs::rename(&tmp, &dst)?;
        }
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `2026-11-13`, or with the time `2026-11-13-153012`, in local time.
fn local_stamp(secs: i64, with_time: bool) -> String {
    let t: libc::time_t = secs as libc::time_t;
    // SAFETY: an all-zero `tm` is valid; localtime_r fills it from `t` only.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    let day = format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
    if with_time { format!("{day}-{:02}{:02}{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec) } else { day }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_are_made_daily_kept_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places { config: dir.path().join("config"), data: dir.path().join("data"), backups: dir.path().join("data/backups") };
        std::fs::create_dir_all(places.data.join("benches/work")).unwrap();
        std::fs::create_dir_all(&places.config).unwrap();
        std::fs::write(places.config.join("shed.toml"), "[font]\nsize = 15\n").unwrap();
        std::fs::write(places.config.join("shell-aliases.sh"), "alias g=git\n").unwrap();
        std::fs::write(places.data.join("benches/work.toml"), "name = \"work\"\n").unwrap();
        std::fs::write(places.data.join("benches/work/scrollback"), "lots").unwrap();
        std::fs::write(places.data.join("session.toml"), "benches = []\n").unwrap();

        let made = daily(&places).unwrap().expect("the day's first");
        assert!(daily(&places).unwrap().is_none(), "one a day");
        assert_eq!(std::fs::read_to_string(made.join("config/shed.toml")).unwrap(), "[font]\nsize = 15\n");
        assert!(made.join("config/shell-aliases.sh").is_file());
        assert!(made.join("data/benches/work.toml").is_file());
        assert!(made.join("data/session.toml").is_file());
        assert!(!made.join("data/benches/work").exists(), "no scrollback");
        assert!(!made.join("data/backups").exists());

        // Broken, then restored; what was there is kept first.
        std::fs::write(places.config.join("shed.toml"), "garbage [[").unwrap();
        std::fs::remove_file(places.data.join("benches/work.toml")).unwrap();
        let name = made.file_name().unwrap().to_str().unwrap().to_string();
        let before = restore(&places, &name).unwrap();
        assert_eq!(std::fs::read_to_string(places.config.join("shed.toml")).unwrap(), "[font]\nsize = 15\n");
        assert!(places.data.join("benches/work.toml").is_file());
        assert_eq!(std::fs::read_to_string(before.join("config/shed.toml")).unwrap(), "garbage [[");
        assert!(restore(&places, "../x").is_err());

        // Only the newest stay.
        for n in 0..KEEP + 3 {
            make(&places, &format!("2000-01-{n:02}")).unwrap();
        }
        prune(&places);
        let left = list(&places);
        assert_eq!(left.len(), KEEP);
        assert!(left.contains(&name), "today's is among the newest");
    }
}
