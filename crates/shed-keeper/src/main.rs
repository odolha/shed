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

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// The server end of a remote tab: joins this terminal to the shell kept
    /// for ID, starting the keeper and the shell if needed.
    Attach {
        id: String,
        /// Where a new shell starts.
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// The remote shells kept here, for other PCs to join: a line each.
    List,
    /// A copy of the command history here (SQLite), to stdout.
    Logbook,
}

/// How long a picture from an AI prompt is kept after it was last shown.
const PICTURES_KEPT: std::time::Duration = std::time::Duration::from_secs(60 * 24 * 3600);
/// Files uploaded for AI prompts are deleted after this long unused.
const UPLOADS_KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        Some(Cmd::List) => {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            match rt.block_on(shed_keeper::attach::list()) {
                Ok(text) => print!("{text}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Some(Cmd::Logbook) => return shed_keeper::attach::logbook(),
        _ => {}
    }
    if let Some(Cmd::Attach { id, cwd }) = args.command {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let code = match rt.block_on(shed_keeper::attach::attach(&id, cwd)) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("shedd attach: {e}\r");
                shed_keeper::attach::EXIT_LOST
            }
        };
        // Exiting now: the runtime would wait on its stdin reader forever.
        std::process::exit(code);
    }
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
        // A backup of your setup each day (at start, and when the day turns).
        tokio::spawn(async {
            loop {
                match tokio::task::spawn_blocking(|| shed_save::backup::daily(&shed_save::backup::Places::default())).await {
                    Ok(Ok(Some(made))) => info!(backup = %made.display(), "backed up the setup"),
                    Ok(Err(e)) => tracing::warn!("cannot back up the setup: {e}"),
                    _ => {}
                }
                // Pictures from AI prompts and screenshots, unseen for 60 days.
                let pictures = paths::state_dir().join("ai-pictures");
                if let Ok(gone) = tokio::task::spawn_blocking(move || shed_ai::prune_pictures(&pictures, PICTURES_KEPT)).await
                    && gone > 0
                {
                    info!(gone, "old AI pictures deleted");
                }
                // Files uploaded for prompts (from the web or Android app), unused for 30 days.
                let uploads = paths::state_dir().join("ai-files");
                if let Ok(gone) = tokio::task::spawn_blocking(move || shed_ai::prune_files(&uploads, UPLOADS_KEPT)).await
                    && gone > 0
                {
                    info!(gone, "old uploaded AI files deleted");
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });

        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let keeper = Keeper::new(KeeperConfig {
            socket: socket.clone(),
            config_file: shed_model::config::Config::path(),
            logbook: Some(shed_logbook::default_path()),
            state_dir: paths::state_dir(),
            shed_bin: std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("shed"))).filter(|p| p.exists()),
            env: Vec::new(),
            store: Some(shed_save::Store::default()),
            desktop: Some(shed_desktop::Desktop::new(paths::data_home())),
            aliases_file: shed_model::config::aliases_path(),
            shell: args.shell,
            home,
            exit_when_idle: !args.persist,
            link_secret: Some(paths::nexus_secret_path()),
            link_log: paths::link_log_path(),
        });

        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
        let stopping = keeper.clone();
        tokio::select! {
            r = keeper.serve(listener) => r?,
            // Logout and shutdown: save every bench before anything else goes.
            _ = term.recv() => { info!("terminated"); stopping.finish(); }
            _ = hup.recv() => { info!("hung up"); stopping.finish(); }
            _ = tokio::signal::ctrl_c() => { info!("interrupted"); stopping.finish(); }
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
