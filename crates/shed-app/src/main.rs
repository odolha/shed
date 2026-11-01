//! `shed`: the bench windows, plus the CLI.

mod actions;
mod assets;
mod bench_window;
mod keeper_conn;
mod line_editor;
mod settings;
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
enum TagAction {
    /// Add a tag, or change its colour.
    Add {
        label: String,
        /// A name (red, orange, yellow, green, blue, violet, teal, grey) or #rrggbb.
        #[arg(long)]
        color: Option<String>,
        /// Tint the whole tab.
        #[arg(long)]
        tint: bool,
    },
    Remove {
        label: String,
    },
    List,
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
    /// Open a bench by name, in its window.
    Load { name: String },
    /// Tag the current tab (run inside Shed).
    Tag {
        #[command(subcommand)]
        action: TagAction,
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
        Some(Command::Load { name }) => {
            let tree = request(ClientMsg::Status, |m| match m {
                KeeperMsg::Status(s) => Some(s.tree),
                _ => None,
            })
            .ok()
            .unwrap_or_default();
            match tree.benches.iter().find(|b| b.name == name) {
                Some(bench) => run_app(ShowTarget::Bench(bench.id)),
                // Saved benches arrive in phase 5; until then, a launcher for a
                // bench the keeper no longer holds starts it afresh.
                None => run_app(ShowTarget::NewBench { name: Some(name), cwd: None }),
            }
        }
        Some(Command::Status) => status(),
        Some(Command::Tag { action }) => tag(action),
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
            let settings = settings::Settings::load(&settings::Settings::default());
            let state = cx.new(|_| AppState::new(conn, settings.clone()));
            state.update(cx, |state, cx| state.apply_settings(settings, cx));
            let receiving = state.clone();
            cx.spawn(async move |cx| AppState::receive(receiving, rx, cx).await).detach();
            cx.spawn(async move |cx| AppState::watch_config(state, cx).await).detach();
            if terminal_view::perf::enabled() {
                cx.spawn(async move |cx| loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    eprintln!("perf: {}", terminal_view::perf::report());
                })
                .detach();
            }
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

fn tag(action: TagAction) -> anyhow::Result<()> {
    use shed_model::{Tag, TabId, default_tag_color, tag_color};
    let Some(tab) = std::env::var("SHED_TAB").ok().and_then(|t| t.parse().ok()).map(TabId) else {
        bail!("not inside a Shed tab ($SHED_TAB is not set)");
    };
    let tree = request(ClientMsg::Status, |m| match m {
        KeeperMsg::Status(s) => Some(s.tree),
        _ => None,
    })?;
    let Some(current) = tree.tab(tab) else { bail!("this tab is no longer in the keeper") };
    let mut tags = current.tags.clone();
    match action {
        TagAction::List => {
            for t in current.all_tags() {
                let auto = if current.tags.iter().any(|m| m.label == t.label) { "" } else { " (rule)" };
                println!("{} #{:06X}{}{auto}", t.label, t.color.to_hex(), if t.tint_tab { " tint" } else { "" });
            }
            return Ok(());
        }
        TagAction::Add { label, color, tint } => {
            let color = match color {
                Some(c) => tag_color(&c).ok_or_else(|| anyhow::anyhow!("unknown colour {c:?}"))?,
                None => default_tag_color(&label),
            };
            tags.retain(|t| t.label != label);
            tags.push(Tag { label, color, tint_tab: tint });
        }
        TagAction::Remove { label } => {
            let before = tags.len();
            tags.retain(|t| t.label != label);
            if tags.len() == before {
                bail!("this tab has no tag {label:?}");
            }
        }
    }
    let (conn, _rx) = keeper_conn::connect(ClientKind::Cli, false)?;
    conn.send_msg(ClientMsg::SetTags { tab, tags });
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
