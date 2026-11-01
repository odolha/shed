//! The logbook: every command run in Shed, kept once and filtered by scope.
//!
//! Each entry carries its pane, tab and bench, so the three histories are
//! just filters over one table: **Local** (this tab), **Bench** (every tab
//! of this bench) and **Global** (everything). Entries from private tabs only
//! ever show up in their own tab.
//!
//! The keeper writes; windows and the shell hook read the same file directly
//! (SQLite in WAL mode allows that), so walking history never waits on the
//! keeper.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use shed_model::{BenchId, TabId, paths};

/// Shed's bash integration, loaded into every bash that Shed starts and
/// printed by `shed init bash`.
pub const BASH_HOOK: &str = include_str!("../../../shell/shed.bash");

pub use rusqlite::Error;

/// `~/.local/state/shed/logbook.sqlite`
pub fn default_path() -> PathBuf {
    paths::state_dir().join("logbook.sqlite")
}

/// A command to record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewEntry {
    pub command: String,
    pub cwd: String,
    /// `None` when the shell did not report one.
    pub exit: Option<i32>,
    /// Unix time in milliseconds.
    pub started_ms: i64,
    pub duration_ms: Option<i64>,
    pub pane: String,
    pub tab: String,
    pub tab_title: String,
    pub bench: String,
    pub tags: Vec<String>,
    pub host: String,
    pub private: bool,
}

/// A recorded command.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub command: String,
    pub cwd: String,
    pub exit: Option<i32>,
    pub started_ms: i64,
    pub duration_ms: Option<i64>,
    pub tab: String,
    pub tab_title: String,
    pub bench: String,
    pub tags: Vec<String>,
}

/// How wide a history to look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// This tab only.
    Local,
    /// Every tab in this bench.
    Bench,
    /// Everything.
    Global,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        Some(match s {
            "local" => Scope::Local,
            "bench" => Scope::Bench,
            "global" => Scope::Global,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Bench => "bench",
            Scope::Global => "global",
        }
    }

    /// The next scope, for cycling in the search overlay.
    pub fn next(self) -> Scope {
        match self {
            Scope::Local => Scope::Bench,
            Scope::Bench => Scope::Global,
            Scope::Global => Scope::Local,
        }
    }
}

/// Where a query is asked from: the tab and bench the scope is relative to.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub tab: TabId,
    pub bench: BenchId,
}

/// One step of walking history: the command at `pos` (1 = most recent) of
/// `total` distinct commands, and the tab it was last run in.
#[derive(Clone, Debug, PartialEq)]
pub struct Walked {
    pub command: String,
    pub pos: usize,
    pub total: usize,
    pub tab: String,
    pub tab_title: String,
}

pub struct Logbook {
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id          INTEGER PRIMARY KEY,
    command     TEXT NOT NULL,
    cwd         TEXT NOT NULL,
    exit        INTEGER,
    started_ms  INTEGER NOT NULL,
    duration_ms INTEGER,
    pane        TEXT NOT NULL,
    tab         TEXT NOT NULL,
    tab_title   TEXT NOT NULL,
    bench       TEXT NOT NULL,
    -- Tag labels as ',a,b,' so one tag matches with LIKE '%,a,%'.
    tags        TEXT NOT NULL,
    host        TEXT NOT NULL,
    private     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS entries_tab ON entries (tab, id);
CREATE INDEX IF NOT EXISTS entries_bench ON entries (bench, id);
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    command, content='entries', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS entries_fts_insert AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts (rowid, command) VALUES (new.id, new.command);
END;
";

impl Logbook {
    /// Opens (creating if needed) the logbook for writing.
    pub fn open(path: &Path) -> Result<Logbook, Error> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Logbook { conn })
    }

    /// Opens an existing logbook for reading.
    pub fn open_readonly(path: &Path) -> Result<Logbook, Error> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(Duration::from_millis(500))?;
        Ok(Logbook { conn })
    }

    pub fn insert(&self, e: &NewEntry) -> Result<i64, Error> {
        let tags = if e.tags.is_empty() { String::new() } else { format!(",{},", e.tags.join(",")) };
        self.conn.execute(
            "INSERT INTO entries (command, cwd, exit, started_ms, duration_ms, pane, tab, tab_title, bench, tags, host, private)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                e.command, e.cwd, e.exit, e.started_ms, e.duration_ms, e.pane, e.tab, e.tab_title, e.bench, tags,
                e.host, e.private
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The `pos`-th most recent distinct command in `scope` that starts with
    /// `prefix` (clamped to the oldest), or `None` if nothing matches.
    pub fn walk(&self, scope: Scope, cx: Context, prefix: &str, pos: usize) -> Result<Option<Walked>, Error> {
        let filter = scope_filter(scope);
        let (tab, bench) = (cx.tab.to_string(), cx.bench.to_string());
        let count_sql = format!(
            "SELECT COUNT(DISTINCT command) FROM entries
             WHERE {filter} AND substr(command, 1, length(:prefix)) = :prefix"
        );
        let all: [(&str, &dyn rusqlite::ToSql); 3] = [(":tab", &tab), (":bench", &bench), (":prefix", &prefix)];
        let total: i64 = self.conn.query_row(&count_sql, used(&count_sql, &all).as_slice(), |r| r.get(0))?;
        let total = total as usize;
        if total == 0 {
            return Ok(None);
        }
        let pos = pos.clamp(1, total);
        // `tab` and `tab_title` come from the row holding MAX(id): SQLite's
        // bare-column rule for aggregate queries.
        let sql = format!(
            "SELECT command, MAX(id), tab, tab_title FROM entries
             WHERE {filter} AND substr(command, 1, length(:prefix)) = :prefix
             GROUP BY command ORDER BY MAX(id) DESC LIMIT 1 OFFSET :offset"
        );
        let offset = (pos - 1) as i64;
        let mut params = used(&sql, &all);
        params.push((":offset", &offset));
        self.conn
            .query_row(&sql, params.as_slice(), |r| {
                Ok(Walked { command: r.get(0)?, pos, total, tab: r.get(2)?, tab_title: r.get(3)? })
            })
            .optional()
    }

    /// Distinct commands in `scope` containing `query` (and tagged `tag`),
    /// most recent first, each with its latest run.
    pub fn search(&self, scope: Scope, cx: Context, query: &str, tag: Option<&str>, limit: usize) -> Result<Vec<Entry>, Error> {
        let filter = scope_filter(scope);
        let query = query.trim();
        // The trigram index needs three characters; shorter queries scan.
        let (text_filter, needle) = if query.chars().count() >= 3 {
            ("id IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH :needle)", format!("\"{}\"", query.replace('"', "\"\"")))
        } else if query.is_empty() {
            ("1", String::new())
        } else {
            ("instr(command, :needle) > 0", query.to_string())
        };
        let tag_filter = if tag.is_some() { "tags LIKE :tag" } else { "1" };
        let sql = format!(
            "SELECT MAX(id), command, cwd, exit, started_ms, duration_ms, tab, tab_title, bench, tags FROM entries
             WHERE {filter} AND {text_filter} AND {tag_filter}
             GROUP BY command ORDER BY MAX(id) DESC LIMIT :limit"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let tab = cx.tab.to_string();
        let bench = cx.bench.to_string();
        let tag_like = tag.map(|t| format!("%,{t},%")).unwrap_or_default();
        let limit = limit as i64;
        let all: [(&str, &dyn rusqlite::ToSql); 5] =
            [(":tab", &tab), (":bench", &bench), (":limit", &limit), (":needle", &needle), (":tag", &tag_like)];
        let rows = stmt.query_map(used(&sql, &all).as_slice(), |r| {
            let tags: String = r.get(9)?;
            Ok(Entry {
                id: r.get(0)?,
                command: r.get(1)?,
                cwd: r.get(2)?,
                exit: r.get(3)?,
                started_ms: r.get(4)?,
                duration_ms: r.get(5)?,
                tab: r.get(6)?,
                tab_title: r.get(7)?,
                bench: r.get(8)?,
                tags: tags.split(',').filter(|t| !t.is_empty()).map(String::from).collect(),
            })
        })?;
        rows.collect()
    }
}

/// The named parameters that `sql` refers to; SQLite rejects any others.
fn used<'a>(sql: &str, all: &[(&'a str, &'a dyn rusqlite::ToSql)]) -> Vec<(&'a str, &'a dyn rusqlite::ToSql)> {
    all.iter().filter(|(name, _)| sql.contains(name)).copied().collect()
}

/// SQL restricting entries to a scope, relative to `:tab` and `:bench`.
/// A private tab's entries are visible only from that tab.
fn scope_filter(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "tab = :tab",
        Scope::Bench => "bench = :bench AND (private = 0 OR tab = :tab)",
        Scope::Global => "(private = 0 OR tab = :tab)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        book: Logbook,
        path: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logbook.sqlite");
        let book = Logbook::open(&path).unwrap();
        Fixture { _dir: dir, book, path }
    }

    fn entry(command: &str, tab: TabId, bench: BenchId) -> NewEntry {
        NewEntry {
            command: command.into(),
            cwd: "/".into(),
            exit: Some(0),
            tab: tab.to_string(),
            tab_title: format!("tab-{}", &tab.to_string()[..4]),
            bench: bench.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn scopes_filter_by_tab_and_bench() {
        let f = fixture();
        let (b1, b2) = (BenchId::new(), BenchId::new());
        let (t1, t2, t3) = (TabId::new(), TabId::new(), TabId::new());
        f.book.insert(&entry("ls tab1", t1, b1)).unwrap();
        f.book.insert(&entry("ls tab2", t2, b1)).unwrap();
        f.book.insert(&entry("ls tab3", t3, b2)).unwrap();
        let cx = Context { tab: t1, bench: b1 };
        let walk = |scope| {
            let total = f.book.walk(scope, cx, "", 1).unwrap().map_or(0, |w| w.total);
            (1..=total).map(|p| f.book.walk(scope, cx, "", p).unwrap().unwrap().command).collect::<Vec<_>>()
        };
        assert_eq!(walk(Scope::Local), ["ls tab1"]);
        assert_eq!(walk(Scope::Bench), ["ls tab2", "ls tab1"]);
        assert_eq!(walk(Scope::Global), ["ls tab3", "ls tab2", "ls tab1"]);
    }

    #[test]
    fn walking_is_distinct_recent_first_with_prefix_and_clamped() {
        let f = fixture();
        let (b, t) = (BenchId::new(), TabId::new());
        for c in ["git status", "cargo build", "git push", "git status", "ls"] {
            f.book.insert(&entry(c, t, b)).unwrap();
        }
        let cx = Context { tab: t, bench: b };
        let at = |prefix, pos| f.book.walk(Scope::Local, cx, prefix, pos).unwrap();
        let first = at("git", 1).unwrap();
        assert_eq!((first.command.as_str(), first.pos, first.total), ("git status", 1, 2));
        assert_eq!(at("git", 2).unwrap().command, "git push");
        let past_end = at("git", 9).unwrap();
        assert_eq!((past_end.command.as_str(), past_end.pos), ("git push", 2));
        assert!(at("docker", 1).is_none());
        assert_eq!(at("", 1).unwrap().command, "ls");
    }

    #[test]
    fn private_tabs_stay_local() {
        let f = fixture();
        let b = BenchId::new();
        let (secret_tab, other_tab) = (TabId::new(), TabId::new());
        f.book.insert(&NewEntry { private: true, ..entry("export TOKEN=x", secret_tab, b) }).unwrap();
        let from_other = Context { tab: other_tab, bench: b };
        let from_secret = Context { tab: secret_tab, bench: b };
        assert!(f.book.walk(Scope::Bench, from_other, "", 1).unwrap().is_none());
        assert!(f.book.walk(Scope::Global, from_other, "", 1).unwrap().is_none());
        assert!(f.book.walk(Scope::Bench, from_secret, "", 1).unwrap().is_some());
    }

    #[test]
    fn search_by_text_and_tag() {
        let f = fixture();
        let (b, t) = (BenchId::new(), TabId::new());
        f.book.insert(&entry("kubectl get pods", t, b)).unwrap();
        f.book.insert(&NewEntry { tags: vec!["prod".into()], ..entry("kubectl delete pod x", t, b) }).unwrap();
        f.book.insert(&entry("ls -la", t, b)).unwrap();
        let cx = Context { tab: t, bench: b };
        let found = |q, tag| {
            f.book.search(Scope::Global, cx, q, tag, 10).unwrap().into_iter().map(|e| e.command).collect::<Vec<_>>()
        };
        assert_eq!(found("pod", None), ["kubectl delete pod x", "kubectl get pods"]);
        assert_eq!(found("pod", Some("prod")), ["kubectl delete pod x"]);
        assert_eq!(found("la", None), ["ls -la"]);
        assert_eq!(found("", None).len(), 3);
        assert_eq!(found("\"quoted", None).len(), 0);
    }

    #[test]
    fn readers_see_writes_and_stay_fast() {
        let f = fixture();
        let (b, t) = (BenchId::new(), TabId::new());
        let tx = f.book.conn.unchecked_transaction().unwrap();
        for i in 0..100_000 {
            f.book.insert(&entry(&format!("command number {}", i % 20_000), t, b)).unwrap();
        }
        tx.commit().unwrap();
        let reader = Logbook::open_readonly(&f.path).unwrap();
        let other = Context { tab: TabId::new(), bench: BenchId::new() };
        let started = std::time::Instant::now();
        let w = reader.walk(Scope::Global, other, "command number 1", 1).unwrap().unwrap();
        let hits = reader.search(Scope::Global, other, "number 1999", None, 50).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(w.command, "command number 19999");
        assert!(!hits.is_empty());
        assert!(elapsed < Duration::from_millis(100), "walk + search took {elapsed:?}");
    }
}
