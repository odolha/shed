//! `shed`: the bench windows, plus the CLI.

mod bench_window;
mod keeper_conn;
mod state;
mod terminal_view;
mod theme;

use std::time::Duration;

use anyhow::bail;
use clap::{Parser, Subcommand};
use gpui::{App, AppContext, QuitMode};
use shed_proto::{ClientKind, ClientMsg, Frame, KeeperMsg};
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
        None => run_app(),
        Some(Command::Status) => status(),
        Some(Command::Stop) => stop(),
        Some(Command::DebugType { text }) => debug_type(&text),
    }
}

fn run_app() -> anyhow::Result<()> {
    let (conn, rx) = keeper_conn::connect(ClientKind::Window, true)?;
    conn.send_msg(ClientMsg::Attach { bench: None });

    gpui_platform::application().with_quit_mode(QuitMode::LastWindowClosed).run(move |cx: &mut App| {
        let state = cx.new(|_| AppState::new(conn));
        cx.spawn(async move |cx| AppState::receive(state, rx, cx).await).detach();
        cx.activate(true);
    });
    Ok(())
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
    println!("keeper: running, pid {}, version {}, up {}s, {} client(s)", s.pid, s.version, s.uptime_secs, s.clients);
    for bench in &s.tree.benches {
        let color = bench.color.name().unwrap_or("custom");
        println!("bench {} ({color}), {} tab(s)", bench.name, bench.tabs.len());
        for tab in &bench.tabs {
            println!("  tab {}: {} pane(s)", tab.title, tab.panes.len());
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
