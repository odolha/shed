//! Blocking requests to the keeper, and the CLI commands that only talk to it
//! (`shed status`, `list`, `stop`, `hide`, `save`, `remote new`).

use std::time::{Duration, Instant};

use anyhow::bail;
use shed_model::{BenchId, Shed, TabId};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, KeeperStatus};

use crate::keeper_conn;

/// Waits up to `timeout` for the next frame: `None` if none came in time.
pub fn recv_timeout(rx: &async_channel::Receiver<Frame>, timeout: Duration) -> Option<Result<Frame, async_channel::RecvError>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(f) => return Some(Ok(f)),
            Err(async_channel::TryRecvError::Closed) => return Some(Err(async_channel::RecvError)),
            Err(async_channel::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    None
}

/// Sends one message and waits for the first reply `pick` accepts.
pub fn request<T>(msg: ClientMsg, mut pick: impl FnMut(KeeperMsg) -> Option<T>) -> anyhow::Result<T> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send_msg(msg);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.recv_blocking() {
            Ok(Frame::Keeper(KeeperMsg::Error(e))) => bail!("keeper: {e}"),
            Ok(Frame::Keeper(msg)) => {
                if let Some(v) = pick(msg) {
                    return Ok(v);
                }
            }
            Ok(_) => {}
            Err(_) => bail!("the keeper closed the connection"),
        }
    }
    bail!("the keeper did not answer")
}

/// The keeper's status.
pub fn status_info() -> anyhow::Result<KeeperStatus> {
    request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s),
        _ => None,
    })
}

/// The keeper's tree.
pub fn tree() -> anyhow::Result<Shed> {
    request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })
}

/// Sends `msg` and waits for the keeper's `Done` text.
pub fn request_done(msg: ClientMsg) -> anyhow::Result<String> {
    request(msg, |m| match m {
        KeeperMsg::Done(text) => Some(text),
        _ => None,
    })
}

/// The running bench named `name`.
fn running_bench(tree: &Shed, name: &str) -> anyhow::Result<BenchId> {
    let Some(bench) = tree.benches.iter().find(|b| b.name == name).map(|b| b.id) else {
        bail!("no running bench named {name:?}");
    };
    Ok(bench)
}

/// The bench named `name`, else the one this process runs in (from
/// `$SHED_BENCH`, or the bench its tab has moved to since).
pub fn bench_named_or_here(tree: &Shed, name: Option<String>) -> anyhow::Result<BenchId> {
    match name {
        Some(name) => tree.benches.iter().find(|b| b.name == name).map(|b| b.id).ok_or_else(|| anyhow::anyhow!("no running bench named {name:?}")),
        None => {
            let Some(id) = std::env::var("SHED_BENCH").ok().and_then(|b| b.parse().ok()).map(BenchId) else {
                bail!("not inside a Shed tab ($SHED_BENCH is not set); name a bench with --bench");
            };
            // The tab may have moved to another bench since its shell started.
            let tab = std::env::var("SHED_TAB").ok().and_then(|t| t.parse().ok()).map(TabId);
            Ok(tab.and_then(|t| tree.bench_of_tab(t)).unwrap_or(id))
        }
    }
}

/// `shed hide NAME`.
pub fn hide(name: &str) -> anyhow::Result<()> {
    let bench = running_bench(&tree()?, name)?;
    println!("{}", request_done(ClientMsg::Hide { bench })?);
    Ok(())
}

/// `shed save NAME [--as COPY]`.
pub fn save(name: &str, as_name: Option<String>) -> anyhow::Result<()> {
    let bench = running_bench(&tree()?, name)?;
    println!("{}", request_done(ClientMsg::SaveBench { bench, as_name })?);
    Ok(())
}

/// `shed status`.
pub fn status() -> anyhow::Result<()> {
    let s = match status_info() {
        Ok(s) => s,
        Err(_) => {
            println!("keeper: not running");
            return Ok(());
        }
    };
    println!(
        "keeper: running, pid {}, version {}, up {}s, {} client(s), {} window process(es)",
        s.pid, s.version, s.uptime_secs, s.clients, s.windows
    );
    if !s.stored.is_empty() {
        println!("closed (saved): {}", s.stored.join(", "));
    }
    for bench in &s.tree.benches {
        let color = bench.color.name().unwrap_or("custom");
        let state = if s.open.contains(&bench.id) { "open" } else { "hidden" };
        println!("bench {} ({color}, {state}), {} tab(s)", bench.name, bench.tabs.len());
        for tab in &bench.tabs {
            let tools: Vec<String> = tab.panes.iter().filter_map(|p| p.tool.as_ref()).map(|t| t.display()).collect();
            let tools = if tools.is_empty() { String::new() } else { format!(", running {}", tools.join(", ")) };
            println!("  tab {}: {} pane(s){tools}", tab.display_title(), tab.panes.len());
        }
    }
    Ok(())
}

/// `shed list`: running benches from the keeper, closed ones from their files.
pub fn list() -> anyhow::Result<()> {
    let status = status_info().ok();
    let saved = shed_save::Store::default().list();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    if let Some(s) = &status {
        for b in &s.tree.benches {
            let state = if s.open.contains(&b.id) { "open" } else { "hidden" };
            rows.push((b.name.clone(), state.into(), format!("{} tab(s)", b.tabs.len())));
        }
    }
    for b in saved {
        if !rows.iter().any(|r| r.0 == b.name) {
            rows.push((b.name, "closed".into(), format!("{} tab(s), saved {}", b.tabs, b.saved.replace('T', " "))));
        }
    }
    if rows.is_empty() {
        println!("No benches.");
    }
    for (name, state, detail) in rows {
        println!("{name:<20} {state:<9} {detail}");
    }
    Ok(())
}

/// `shed stop`: stops the keeper, whatever version of Shed it is from. It is
/// asked first; a keeper from another version may not understand the request
/// (messages travel by position), so if it is still running after a while
/// it gets SIGTERM, on which a keeper saves its benches and exits too.
pub fn stop() -> anyhow::Result<()> {
    let (conn, rx) = match keeper_conn::connect(ClientKind::Cli, false) {
        Ok(c) => c,
        Err(_) => {
            println!("keeper: not running");
            return Ok(());
        }
    };
    conn.send_msg(ClientMsg::Shutdown);
    let Some(pid) = conn.keeper_pid else {
        // No process to watch: the keeper closes the connection as it exits.
        return match recv_timeout(&rx, Duration::from_secs(5)) {
            Some(Err(_)) => {
                println!("keeper: stopped");
                Ok(())
            }
            _ => bail!("the keeper did not stop"),
        };
    };
    let gone = |timeout: Duration| {
        let deadline = Instant::now() + timeout;
        while keeper_conn::alive(pid) {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    };
    // Another version says so at once; it would not understand the request.
    let mut other_version = false;
    while let Some(Ok(frame)) = recv_timeout(&rx, Duration::from_millis(300)) {
        if let Frame::Keeper(KeeperMsg::Error(e)) = frame
            && keeper_conn::is_other_version(&e)
        {
            other_version = true;
            break;
        }
    }
    if other_version || !gone(Duration::from_secs(5)) {
        let why = if other_version { "is from another version of Shed" } else { "did not answer" };
        eprintln!("keeper: pid {pid} {why}; stopping it with SIGTERM");
        // SAFETY: kill(2) has no memory effects; `pid` is the keeper at the
        // other end of the socket, still running a moment ago.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if !gone(Duration::from_secs(10)) {
            bail!("the keeper (pid {pid}) did not stop");
        }
    }
    drop(rx);
    println!("keeper: stopped");
    Ok(())
}

/// `shed debug-type TEXT`: types into the first bench's active pane.
pub fn debug_type(text: &str) -> anyhow::Result<()> {
    let tree = tree()?;
    let Some(pane) = tree.benches.first().and_then(|b| b.active_tab()).map(|t| t.active_pane) else {
        bail!("no panes");
    };
    let (conn, _rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send(Frame::Input { pane, bytes: text.replace("\\r", "\r").replace("\\e", "\x1b").into_bytes() });
    // Give the writer thread a moment to flush before exiting.
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// `shed remote new HOST`: a remote tab in the bench named, else this one.
pub fn remote_new(host: String, bench: Option<String>, join: Option<String>) -> anyhow::Result<()> {
    let bench = bench_named_or_here(&tree()?, bench)?;
    println!("{}", request_done(ClientMsg::NewRemoteTab { bench, host, join })?);
    Ok(())
}
