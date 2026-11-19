//! `shed-nexus`: runs the nexus until it is stopped (Ctrl+C or SIGTERM).

use std::path::PathBuf;

use clap::Parser;
use shed_nexus::config::Config;

/// The nexus: links Shed workspaces to the web and Android apps.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// The settings file. Default: $XDG_CONFIG_HOME/shed-nexus/nexus.toml.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("SHED_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Colours only on a terminal, not in journald.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_ansi(ansi).init();

    let args = Args::parse();
    let path = args.config.unwrap_or_else(Config::default_path);
    let config = Config::load(&path)?;
    tracing::info!("settings from {}; data in {}", path.display(), config.data_dir().display());
    let server = shed_nexus::start(config).await?;

    wait_for_stop().await;
    tracing::info!("stopping");
    server.stop().await;
    Ok(())
}

async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
