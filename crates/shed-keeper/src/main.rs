use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use shed_keeper::{Keeper, KeeperConfig};
use shed_model::paths;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// The Shed keeper: holds every shell and running program.
#[derive(Parser)]
#[command(name = "shedd", version)]
struct Args {
    /// Log to stderr instead of the log file.
    #[arg(long)]
    foreground: bool,

    /// Keep running when no panes and no windows remain.
    #[arg(long)]
    persist: bool,

    /// Shell for new panes (default: $SHELL).
    #[arg(long)]
    shell: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_logging(args.foreground)?;

    let runtime_dir = paths::runtime_dir();
    shed_keeper::ensure_private_dir(&runtime_dir)
        .with_context(|| format!("creating {}", runtime_dir.display()))?;
    let Some(_lock) = shed_keeper::take_lock(&paths::lock_path())? else {
        info!("another keeper is already running");
        return Ok(());
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async {
        let socket = paths::socket_path();
        let listener = shed_keeper::bind_socket(&socket)
            .with_context(|| format!("binding {}", socket.display()))?;
        info!(pid = std::process::id(), socket = %socket.display(), "keeper started");

        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let keeper = Keeper::new(KeeperConfig {
            socket: socket.clone(),
            config_file: shed_model::config::Config::path(),
            shell: args.shell,
            home,
            exit_when_idle: !args.persist,
        });

        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = keeper.serve(listener) => r?,
            _ = term.recv() => info!("terminated"),
            _ = tokio::signal::ctrl_c() => info!("interrupted"),
        }
        let _ = std::fs::remove_file(&socket);
        info!("keeper stopped");
        anyhow::Ok(())
    })?;
    // Pane tasks may still hold PTYs; exiting closes them and hangs up their sessions.
    rt.shutdown_timeout(std::time::Duration::from_millis(100));
    Ok(())
}

fn init_logging(foreground: bool) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SHED_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if foreground {
        builder.with_writer(std::io::stderr).init();
    } else {
        let path = paths::keeper_log_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
    }
    Ok(())
}
