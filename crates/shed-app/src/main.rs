//! `shed`: the bench windows, plus the CLI.

mod actions;
mod assets;
mod bench_window;
mod keeper_conn;
mod line_editor;
mod state;
mod terminal_view;
mod theme;

use std::time::Duration;

use anyhow::bail;
use clap::{Parser, Subcommand};
use gpui::{App, AppContext, QuitMode};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg, PROTOCOL_VERSION, ShowTarget};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[derive(Parser)]
#[command(name = "shed", version, about = "A terminal whose shells outlive its windows")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open a new bench in its own window.
    New {
        /// The bench's name (default: the next free one).
        #[arg(long)]
        name: Option<String>,
        /// The bench's directory (default: home).
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
    },
    /// Show whether the keeper is running, and what it holds.
    Status,
    /// Stop the keeper and every tool it holds.
    Stop,
    /// Types text into the first bench's active pane (for testing; `\r` is Enter).
    #[command(hide = true)]
    DebugType { text: String },
}

fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SHED_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();

    match Cli::parse().command {
        None => run_app(ShowTarget::Detached),
        Some(Command::New { name, cwd }) => {
            let cwd = cwd.map(std::path::absolute).transpose()?;
            run_app(ShowTarget::NewBench { name, cwd })
        }
        Some(Command::Status) => status(),
        Some(Command::Stop) => stop(),
        Some(Command::DebugType { text }) => debug_type(&text),
    }
}

/// Shows `target`. If a window process is already running, it shows it and
/// this process exits; otherwise this process becomes the window process.
fn run_app(target: ShowTarget) -> anyhow::Result<()> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Cli, true)?;
    conn.send_msg(ClientMsg::Status);
    let windows = loop {
        match rx.recv_blocking() {
            Ok(Frame::Keeper(KeeperMsg::Status(s))) => break s.windows,
            Ok(Frame::Keeper(KeeperMsg::Error(e))) => bail!("keeper: {e}. Run `shed stop` to restart it."),
            Ok(_) => {}
            Err(_) => bail!("the keeper closed the connection"),
        }
    };
    if windows > 0 {
        conn.send_msg(ClientMsg::Show(target));
        // Errors come back at once; silence means the window process has it.
        if let Ok(Ok(Frame::Keeper(KeeperMsg::Error(e)))) =
            futures_lite_timeout(&rx, Duration::from_millis(300))
        {
            bail!("keeper: {e}");
        }
        return Ok(());
    }

    conn.send_msg(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Window });
    conn.send_msg(ClientMsg::Show(target));
    gpui_platform::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
            actions::bind_default_keys(cx);
            let state = cx.new(|_| AppState::new(conn));
            cx.spawn(async move |cx| AppState::receive(state, rx, cx).await).detach();
            cx.activate(true);
        });
    Ok(())
}

/// Waits up to `timeout` for the next frame.
fn futures_lite_timeout(
    rx: &async_channel::Receiver<Frame>,
    timeout: Duration,
) -> Result<Result<Frame, async_channel::RecvError>, ()> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(f) => return Ok(Ok(f)),
            Err(async_channel::TryRecvError::Closed) => return Ok(Err(async_channel::RecvError)),
            Err(async_channel::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    Err(())
}

/// Sends one message and waits for the first reply `pick` accepts.
fn request<T>(msg: ClientMsg, mut pick: impl FnMut(KeeperMsg) -> Option<T>) -> anyhow::Result<T> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send_msg(msg);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
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

fn status() -> anyhow::Result<()> {
    let s = match request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s),
        _ => None,
    }) {
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
    for bench in &s.tree.benches {
        let color = bench.color.name().unwrap_or("custom");
        let state = if s.open.contains(&bench.id) { "open" } else { "detached" };
        println!("bench {} ({color}, {state}), {} tab(s)", bench.name, bench.tabs.len());
        for tab in &bench.tabs {
            let tools: Vec<String> = tab.panes.iter().filter_map(|p| p.tool.as_ref()).map(|t| t.display()).collect();
            let tools = if tools.is_empty() { String::new() } else { format!(", running {}", tools.join(", ")) };
            println!("  tab {}: {} pane(s){tools}", tab.display_title(), tab.panes.len());
        }
    }
    Ok(())
}

fn stop() -> anyhow::Result<()> {
    let (conn, rx) = match keeper_conn::connect(ClientKind::Cli, false) {
        Ok(c) => c,
        Err(_) => {
            println!("keeper: not running");
            return Ok(());
        }
    };
    conn.send_msg(ClientMsg::Shutdown);
    // The keeper closes the connection as it exits.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if rx.recv_blocking().is_err() {
            println!("keeper: stopped");
            return Ok(());
        }
    }
    bail!("the keeper did not stop")
}

fn debug_type(text: &str) -> anyhow::Result<()> {
    let tree = request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })?;
    let Some(pane) = tree.benches.first().and_then(|b| b.active_tab()).map(|t| t.active_pane) else {
        bail!("no panes");
    };
    let (conn, _rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send(Frame::Input { pane, bytes: text.replace("\\r", "\r").replace("\\e", "\x1b").into_bytes() });
    // Give the writer thread a moment to flush before exiting.
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
