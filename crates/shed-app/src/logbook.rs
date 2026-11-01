//! The logbook from the command line and from the shell hook.

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use base64::Engine;
use shed_logbook::{Context, Logbook, Scope};
use shed_model::{BenchId, TabId};

/// The logbook the keeper writes: `$SHED_STATE_DIR` (set in Shed's panes) or the default.
pub fn path() -> PathBuf {
    match std::env::var_os("SHED_STATE_DIR") {
        Some(dir) => PathBuf::from(dir).join("logbook.sqlite"),
        None => shed_logbook::default_path(),
    }
}

/// The tab and bench this process runs in, from the variables Shed sets.
fn here() -> Option<Context> {
    let tab = std::env::var("SHED_TAB").ok()?.parse().ok().map(TabId)?;
    let bench = std::env::var("SHED_BENCH").ok()?.parse().ok().map(BenchId)?;
    Some(Context { tab, bench })
}

/// One step of a history walk, printed for the shell hook. Returns whether
/// there was anything to print.
pub fn hook_walk(scope: &str, pos: usize, prefix: &str) -> anyhow::Result<bool> {
    let scope = Scope::parse(scope).context("unknown scope")?;
    let cx = here().context("not inside Shed")?;
    let book = Logbook::open_readonly(&path())?;
    let Some(w) = book.walk(scope, cx, prefix, pos)? else { return Ok(false) };
    let source = if w.tab == cx.tab.to_string() { String::new() } else { w.tab_title };
    let source = base64::engine::general_purpose::STANDARD.encode(source);
    println!("{} {} {}", w.pos, w.total, source);
    print!("{}", w.command);
    Ok(true)
}

pub fn print_log(scope: &str, count: usize, tag: Option<&str>, query: Option<&str>) -> anyhow::Result<()> {
    let scope = Scope::parse(scope).context("scope must be local, bench or global")?;
    let cx = match (scope, here()) {
        (_, Some(cx)) => cx,
        (Scope::Global, None) => Context { tab: TabId::new(), bench: BenchId::new() },
        (_, None) => bail!("the {} scope only works inside Shed", scope.name()),
    };
    let book = match Logbook::open_readonly(&path()) {
        Ok(book) => book,
        Err(_) => {
            println!("The logbook is empty.");
            return Ok(());
        }
    };
    let mut entries = book.search(scope, cx, query.unwrap_or(""), tag, count)?;
    entries.reverse();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    for e in entries {
        let status = match e.exit {
            Some(0) => "  ".to_string(),
            Some(code) => format!("{code:>2}"),
            None => " ?".to_string(),
        };
        println!("{status} {:>5}  {:<14}  {}", age(now - e.started_ms), truncate(&e.tab_title, 14), e.command);
    }
    Ok(())
}

/// A short age like `5m`, `3h` or `2d`.
pub fn age(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
